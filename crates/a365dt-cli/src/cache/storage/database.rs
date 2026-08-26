use std::{
	fs::{self, File, OpenOptions},
	io,
	path::{Path, PathBuf},
	sync::Arc,
};

use sqlx::{SqlitePool, migrate::Migrator};

use crate::{
	app_files,
	error::Error,
	sqlite::{self, Durability, FailureContext, MigrationError, OpenMode},
};

pub(super) const FILE: &str = "cache.sqlite";
const LOCK_FILE: &str = "cache.lock";
const INITIALIZATION_LOCK_FILE: &str = "cache-initialization.lock";
const LEGACY_FILES: [&str; 2] = ["series.json", "latest-release.json"];
static MIGRATOR: Migrator = sqlx::migrate!("./migrations/cache");

#[derive(Clone, Debug)]
pub(super) struct Database {
	pub(super) pool: SqlitePool,
	_lock: Arc<FileLock>,
}

pub(super) struct OpenFailure {
	pub(super) error: Error,
	pub(super) rebuildable: bool,
}

#[derive(Debug)]
struct FileLock(File);

impl Drop for FileLock {
	fn drop(&mut self) {
		// Forked children can keep a duplicated descriptor open after this one closes.
		let _ = self.0.unlock();
	}
}

pub(super) async fn open(directory: &Path) -> Result<Database, OpenFailure> {
	fs::create_dir_all(directory).map_err(|error| OpenFailure {
		error: Error::with_debug("Could not open the local cache.", error),
		rebuildable: false,
	})?;
	let path = directory.join(FILE);
	let cache_lock =
		Arc::new(shared_lock(directory).map_err(|error| OpenFailure {
			error,
			rebuildable: false,
		})?);
	let _initialization_lock =
		initialization_lock(directory).map_err(|error| OpenFailure {
			error,
			rebuildable: false,
		})?;
	if !path.exists() {
		match open_database(
			path.clone(),
			Arc::clone(&cache_lock),
			OpenMode::Initialize,
		)
		.await
		{
			Ok(database) => database.pool.close().await,
			Err(failure) => {
				sqlite::remove_new_database(&path);
				return Err(failure);
			}
		}
	}
	open_database(path, cache_lock, OpenMode::Existing).await
}

pub(super) async fn rebuild(directory: &Path) -> Result<(), Error> {
	let cache_lock = exclusive_lock(directory)?;
	let path = directory.join(FILE);
	for candidate in sqlite::files(&path) {
		match fs::remove_file(&candidate) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(Error::with_debug(
					"Could not rebuild the local cache.",
					format!("{}: {error}", candidate.display()),
				));
			}
		}
	}
	let database =
		open_database(path, Arc::new(cache_lock), OpenMode::Initialize)
			.await
			.map_err(|failure| failure.error)?;
	database.pool.close().await;
	Ok(())
}

pub(super) fn retire_legacy_files(directory: &Path) -> Result<(), Error> {
	for file in LEGACY_FILES {
		let path = directory.join(file);
		match fs::remove_file(&path) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(Error::with_debug(
					"Could not retire an obsolete local cache file; it will be ignored.",
					format!("{}: {error}", path.display()),
				));
			}
		}
	}
	Ok(())
}

pub(super) fn size(path: &Path) -> Result<u64, Error> {
	sqlite::size(path)
		.map_err(|error| read_error(format!("{}: {error}", path.display())))
}

async fn open_database(
	path: PathBuf,
	cache_lock: Arc<FileLock>,
	mode: OpenMode,
) -> Result<Database, OpenFailure> {
	let pool = match sqlite::connect(&path, mode, Durability::Cache).await {
		Ok(pool) => pool,
		Err(error) => {
			drop(cache_lock);
			return Err(open_failure(&path, error, FailureContext::Opening));
		}
	};
	if let Err(failure) = migrate(&pool, &path).await {
		pool.close().await;
		drop(cache_lock);
		return Err(failure);
	}
	if let Err(failure) = validate_schema(&pool, &path).await {
		pool.close().await;
		drop(cache_lock);
		return Err(failure);
	}
	if let Err(error) = app_files::private_file(&path) {
		pool.close().await;
		drop(cache_lock);
		return Err(OpenFailure {
			error: Error::with_debug(
				"Could not secure the local cache database.",
				error,
			),
			rebuildable: false,
		});
	}
	Ok(Database {
		pool,
		_lock: cache_lock,
	})
}

async fn migrate(pool: &SqlitePool, path: &Path) -> Result<(), OpenFailure> {
	let (transaction, _) = sqlite::begin_migrations(pool, &MIGRATOR, "cache")
		.await
		.map_err(|error| match error {
			MigrationError::Database(error) => {
				open_failure(path, error, FailureContext::Schema)
			}
			MigrationError::Invalid(detail) => schema_failure(path, detail),
		})?;
	transaction
		.commit()
		.await
		.map_err(|error| open_failure(path, error, FailureContext::Schema))
}

async fn validate_schema(
	pool: &SqlitePool,
	path: &Path,
) -> Result<(), OpenFailure> {
	let state = sqlx::query(
		"SELECT revision, current_generation, last_refresh_revision, \
		 refreshed_at, next_discovery_order, \
		 (SELECT COUNT(*) FROM series), \
		 (SELECT COUNT(*) FROM aliases), \
		 (SELECT COUNT(*) FROM catalogue_source_state), \
		 (SELECT COUNT(*) FROM release) \
		 FROM catalogue_state WHERE singleton = 1",
	)
	.fetch_optional(pool)
	.await
	.map_err(|error| open_failure(path, error, FailureContext::Schema))?;
	if state.is_none() {
		return Err(schema_failure(path, "cache state is missing"));
	}
	Ok(())
}

fn shared_lock(directory: &Path) -> Result<FileLock, Error> {
	let file = lock_file(directory, LOCK_FILE)?;
	file.lock_shared().map_err(|error| {
		Error::with_debug("Could not open the local cache.", error)
	})?;
	Ok(FileLock(file))
}

fn initialization_lock(directory: &Path) -> Result<FileLock, Error> {
	let file = lock_file(directory, INITIALIZATION_LOCK_FILE)?;
	file.lock().map_err(|error| {
		Error::with_debug("Could not initialize the local cache.", error)
	})?;
	Ok(FileLock(file))
}

fn exclusive_lock(directory: &Path) -> Result<FileLock, Error> {
	let file = lock_file(directory, LOCK_FILE)?;
	file.try_lock().map_err(|error| {
		Error::with_debug(
			"Could not rebuild the local cache while it is in use.",
			error,
		)
	})?;
	Ok(FileLock(file))
}

fn lock_file(directory: &Path, name: &str) -> Result<File, Error> {
	fs::create_dir_all(directory).map_err(|error| {
		Error::with_debug("Could not open the local cache.", error)
	})?;
	OpenOptions::new()
		.create(true)
		.truncate(false)
		.read(true)
		.write(true)
		.open(directory.join(name))
		.map_err(|error| {
			Error::with_debug("Could not open the local cache.", error)
		})
}

fn open_failure(
	path: &Path,
	error: sqlx::Error,
	context: FailureContext,
) -> OpenFailure {
	OpenFailure {
		error: Error::with_debug(
			"Could not open the local cache; run `a365 cache prune` to inspect or reset it.",
			format!("{}: {error}", path.display()),
		),
		rebuildable: sqlite::is_structural(&error, context),
	}
}

fn schema_failure(path: &Path, detail: impl std::fmt::Display) -> OpenFailure {
	OpenFailure {
		error: Error::with_debug(
			"Could not open the local cache; run `a365 cache prune` to inspect or reset it.",
			format!("{}: {detail}", path.display()),
		),
		rebuildable: true,
	}
}

fn read_error(error: impl std::fmt::Display) -> Error {
	Error::with_debug(
		"Could not read the local cache; run `a365 cache prune` to reset it.",
		error,
	)
}

#[cfg(test)]
#[path = "database_tests.rs"]
mod tests;
