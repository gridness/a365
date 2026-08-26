use std::{
	fs::{self, File, OpenOptions},
	io,
	path::{Path, PathBuf},
};

use sqlx::SqlitePool;

use super::{
	Paths, recording,
	storage::{self, Store},
};
use crate::{
	error::Error,
	sqlite::{self, Durability, FailureContext, OpenMode},
};

const SQLITE_WAL_DEADMAN_SWITCH_OFFSET: u64 = 128;

#[derive(Clone, Copy)]
pub(crate) enum TelemetryRecovery {
	Enabled,
	Disabled,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum MigrationPreparation {
	Ready,
	Damaged,
}

pub(crate) struct MigrationDatabaseLock {
	file: Option<File>,
	path: PathBuf,
	remove_file: bool,
}

impl MigrationDatabaseLock {
	pub(crate) fn close(mut self) -> Result<(), Error> {
		if self.remove_file {
			self.remove_file = false;
			match fs::remove_file(&self.path) {
				Ok(()) => {}
				Err(error) if error.kind() == io::ErrorKind::NotFound => {}
				Err(error) => return Err(migration_error("release", error)),
			}
		}
		drop(self.file.take());
		Ok(())
	}
}

impl Drop for MigrationDatabaseLock {
	fn drop(&mut self) {
		if self.remove_file {
			let _ = fs::remove_file(&self.path);
			self.remove_file = false;
		}
		drop(self.file.take());
	}
}

pub(crate) async fn prepare_migration_at(
	directory: &Path,
) -> Result<MigrationPreparation, Error> {
	let paths = Paths::at(directory.to_owned());
	if paths.data.exists() && database_is_damaged(&paths.data).await? {
		return Ok(MigrationPreparation::Damaged);
	}
	match fs::read_to_string(&paths.disabled) {
		Ok(timestamp) if invalid_timestamp(&timestamp) => {
			return Ok(MigrationPreparation::Damaged);
		}
		Ok(_) => {}
		Err(error) if error.kind() == io::ErrorKind::NotFound => {}
		Err(error) => return Err(migration_error("inspect", error)),
	}
	let store = Store::open(paths).await?;
	store.close().await;
	Ok(MigrationPreparation::Ready)
}

pub(crate) fn ensure_migration_idle(
	database: &Path,
) -> Result<Option<MigrationDatabaseLock>, Error> {
	if !database
		.try_exists()
		.map_err(|error| migration_error("inspect", error))?
	{
		return Ok(None);
	}
	let path = sqlite::files(database)[2].clone();
	let (file, remove_file) = open_lock_file(&path)?;
	match lock_shared_memory(&file) {
		Ok(()) => Ok(Some(MigrationDatabaseLock {
			file: Some(file),
			path,
			remove_file,
		})),
		Err(error)
			if matches!(
				error.kind(),
				io::ErrorKind::WouldBlock | io::ErrorKind::PermissionDenied
			) =>
		{
			Err(Error::new(
				"Legacy application state is in use; close other a365 processes and retry.",
			))
		}
		Err(error) => Err(migration_error("inspect", error)),
	}
}

pub(crate) async fn recreate_migration_at(
	directory: &Path,
	recovery: TelemetryRecovery,
) -> Result<(), Error> {
	let paths = Paths::at(directory.to_owned());
	for path in sqlite::files(&paths.data) {
		match fs::remove_file(&path) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => return Err(migration_error("recreate", error)),
		}
	}
	match recovery {
		TelemetryRecovery::Enabled => match fs::remove_file(&paths.disabled) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => return Err(migration_error("recreate", error)),
		},
		TelemetryRecovery::Disabled => {
			let disabled_at = (recording::now_ms() / 1_000).to_string();
			fs::write(&paths.disabled, disabled_at)
				.map_err(|error| migration_error("recreate", error))?;
		}
	}
	match prepare_migration_at(directory).await? {
		MigrationPreparation::Ready => Ok(()),
		MigrationPreparation::Damaged => {
			Err(Error::new("Could not recreate the local telemetry."))
		}
	}
}

fn open_lock_file(path: &Path) -> Result<(File, bool), Error> {
	match OpenOptions::new()
		.create_new(true)
		.read(true)
		.write(true)
		.open(path)
	{
		Ok(file) => Ok((file, true)),
		Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
			OpenOptions::new()
				.read(true)
				.write(true)
				.open(path)
				.map(|file| (file, false))
				.map_err(|error| migration_error("inspect", error))
		}
		Err(error) => Err(migration_error("inspect", error)),
	}
}

#[cfg(unix)]
fn lock_shared_memory(file: &File) -> io::Result<()> {
	use std::os::fd::AsRawFd;

	// SAFETY: `libc::flock` is a C record of integer fields, so all-zero is a
	// valid unlocked template before the required fields are assigned below.
	let mut lock = unsafe { std::mem::zeroed::<libc::flock>() };
	lock.l_type = libc::F_WRLCK as _;
	lock.l_whence = libc::SEEK_SET as _;
	lock.l_start = SQLITE_WAL_DEADMAN_SWITCH_OFFSET as _;
	lock.l_len = 1;
	// SAFETY: `lock` is initialized for F_SETLK and the file descriptor is valid.
	if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == -1 {
		Err(io::Error::last_os_error())
	} else {
		Ok(())
	}
}

#[cfg(not(unix))]
fn lock_shared_memory(file: &File) -> io::Result<()> {
	file.try_lock().map_err(|error| match error {
		fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
		fs::TryLockError::Error(error) => error,
	})
}

async fn database_is_damaged(path: &Path) -> Result<bool, Error> {
	let pool =
		match sqlite::connect(path, OpenMode::Existing, Durability::Telemetry)
			.await
		{
			Ok(pool) => pool,
			Err(error)
				if sqlite::is_structural(&error, FailureContext::Opening) =>
			{
				return Ok(true);
			}
			Err(error) => return Err(migration_error("inspect", error)),
		};
	let result = validate_database(&pool).await;
	pool.close().await;
	result
}

async fn validate_database(pool: &SqlitePool) -> Result<bool, Error> {
	let check = match sqlx::query_scalar::<_, String>("PRAGMA quick_check")
		.fetch_all(pool)
		.await
	{
		Ok(check) => check,
		Err(error) => return validation_error(error),
	};
	if check.as_slice() != ["ok"] {
		return Ok(true);
	}
	let applied = match sqlx::query_as::<_, (i64, Vec<u8>, bool)>(
		"SELECT version, checksum, success FROM _sqlx_migrations",
	)
	.fetch_all(pool)
	.await
	{
		Ok(applied) => applied,
		Err(error) => return validation_error(error),
	};
	let expected = storage::MIGRATOR
		.iter()
		.filter(|migration| migration.migration_type.is_up_migration())
		.collect::<Vec<_>>();
	if applied.iter().any(|(version, checksum, success)| {
		!expected.iter().any(|migration| {
			*version == migration.version
				&& *success && checksum.as_slice() == migration.checksum.as_ref()
		})
	}) {
		return Ok(true);
	}
	match sqlx::query_scalar::<_, i64>(
		"SELECT COUNT(*) FROM collection_state WHERE singleton = 1",
	)
	.fetch_one(pool)
	.await
	{
		Ok(1) => Ok(false),
		Ok(_) => Ok(true),
		Err(error) => validation_error(error),
	}
}

fn validation_error(error: sqlx::Error) -> Result<bool, Error> {
	if sqlite::is_structural(&error, FailureContext::Schema) {
		Ok(true)
	} else {
		Err(migration_error("inspect", error))
	}
}

fn invalid_timestamp(timestamp: &str) -> bool {
	timestamp
		.parse::<u64>()
		.ok()
		.and_then(|timestamp| timestamp.checked_mul(1_000))
		.and_then(|timestamp| i64::try_from(timestamp).ok())
		.is_none()
}

fn migration_error(action: &str, error: impl std::fmt::Display) -> Error {
	Error::with_debug(format!("Could not {action} the local telemetry."), error)
}
