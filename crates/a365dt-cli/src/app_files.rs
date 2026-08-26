use std::{
	fs::{self, File, OpenOptions},
	io,
	path::{Path, PathBuf},
};

use directories::{BaseDirs, ProjectDirs};

mod migration;

pub(crate) use migration::prepare_for_command;

pub(crate) const APPLICATION_ID: &str = if cfg!(debug_assertions) {
	"a365-dev"
} else {
	"a365"
};
pub(crate) const LEGACY_APPLICATION_ID: &str = if cfg!(debug_assertions) {
	"a365dt-dev"
} else {
	"a365dt"
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Paths {
	root: PathBuf,
	cache: PathBuf,
	data: PathBuf,
}

enum Preparation {
	Ready,
	Migration(Migration),
}

struct Migration {
	application_home: Paths,
	legacy_roots: Vec<PathBuf>,
	files: Vec<(PathBuf, LegacyFileDisposition)>,
	lock_files: Vec<PathBuf>,
	migration_lock: Option<migration::MigrationLock>,
}

struct StagedMigration {
	staging: Paths,
	application_home: Paths,
	legacy_roots: Vec<PathBuf>,
	_locks: Vec<File>,
	_migration_lock: Option<migration::MigrationLock>,
}

struct LockedMigration {
	migration: Migration,
	locks: Vec<File>,
}

#[derive(Clone, Copy)]
enum LegacyFileDisposition {
	Root,
	Cache,
	Data,
	Discard,
}

impl LegacyFileDisposition {
	fn destination(self, paths: &Paths) -> Option<&Path> {
		match self {
			Self::Root => Some(&paths.root),
			Self::Cache => Some(&paths.cache),
			Self::Data => Some(&paths.data),
			Self::Discard => None,
		}
	}
}

impl Paths {
	fn at(root: PathBuf) -> Self {
		Self {
			cache: root.join("cache"),
			data: root.join("data"),
			root,
		}
	}
}

impl Migration {
	fn legacy_telemetry_database(&self) -> Option<&Path> {
		self.files.iter().find_map(|(path, disposition)| {
			(matches!(disposition, LegacyFileDisposition::Data)
				&& path
					.file_name()
					.is_some_and(|name| name == "telemetry.sqlite"))
			.then_some(path.as_path())
		})
	}

	fn moves(&self) -> Vec<[String; 3]> {
		let mut moves = self
			.files
			.iter()
			.map(|(source, disposition)| {
				let destination = disposition
					.destination(&self.application_home)
					.expect("discarded files are not part of a migration");
				[
					source.parent().unwrap_or(source).display().to_string(),
					"→".to_owned(),
					destination.display().to_string(),
				]
			})
			.collect::<Vec<_>>();
		moves.sort_unstable();
		moves.dedup();
		moves
	}

	fn lock(self) -> io::Result<LockedMigration> {
		let locks = lock_legacy_files(&self.lock_files)?;
		Ok(LockedMigration {
			migration: self,
			locks,
		})
	}
}

impl LockedMigration {
	fn stage(self) -> io::Result<StagedMigration> {
		let staging = staging_paths(&self.migration.application_home)?;
		purge_directories(std::slice::from_ref(&staging.root))?;
		create_paths(&staging)?;
		let config = self.migration.application_home.root.join("config.toml");
		match fs::copy(&config, staging.root.join("config.toml")) {
			Ok(_) => private_file(&staging.root.join("config.toml"))?,
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => return Err(error),
		}
		for (source, disposition) in self.migration.files {
			let Some(destination) = disposition.destination(&staging) else {
				continue;
			};
			let destination = destination.join(source.file_name().unwrap());
			match fs::copy(&source, &destination) {
				Ok(_) => {}
				Err(error)
					if error.kind() == io::ErrorKind::NotFound
						&& source
							.file_name()
							.and_then(|name| name.to_str())
							.is_some_and(|name| {
								name.ends_with("-wal") || name.ends_with("-shm")
							}) => {}
				Err(error) => return Err(error),
			}
			if !destination.exists() {
				continue;
			}
			private_file(&destination)?;
		}
		Ok(StagedMigration {
			staging,
			application_home: self.migration.application_home,
			legacy_roots: self.migration.legacy_roots,
			_locks: self.locks,
			_migration_lock: self.migration.migration_lock,
		})
	}
}

fn lock_legacy_files(paths: &[PathBuf]) -> io::Result<Vec<File>> {
	let mut locks = Vec::new();
	for path in paths {
		let file = match OpenOptions::new().read(true).write(true).open(path) {
			Ok(file) => file,
			Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
			Err(error) => return Err(error),
		};
		file.try_lock().map_err(|error| match error {
			fs::TryLockError::WouldBlock => io::Error::new(
				io::ErrorKind::WouldBlock,
				"Legacy application state is in use; close other a365 or a365dt processes and retry.",
			),
			fs::TryLockError::Error(error) => error,
		})?;
		locks.push(file);
	}
	Ok(locks)
}

impl StagedMigration {
	fn paths(&self) -> &Paths {
		&self.staging
	}

	fn commit(mut self) -> io::Result<Paths> {
		let mut roots = self.legacy_roots.clone();
		roots.push(self.application_home.root.clone());
		let tombstones = migration::move_to_tombstones(&roots)?;
		if let Err(error) =
			fs::rename(&self.staging.root, &self.application_home.root)
		{
			return match migration::restore_tombstones(&tombstones) {
				Ok(()) => Err(error),
				Err(restore_error) => Err(io::Error::other(format!(
					"{error}; could not restore application files: {restore_error}"
				))),
			};
		}
		self._locks.clear();
		purge_directories(
			&tombstones
				.iter()
				.map(|(_, tombstone)| tombstone.clone())
				.collect::<Vec<_>>(),
		)?;
		Ok(self.application_home.clone())
	}
}

impl Drop for StagedMigration {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.staging.root);
	}
}

fn prepare_at(
	paths: Paths,
	legacy_roots: &[PathBuf],
) -> io::Result<Preparation> {
	let mut files = Vec::new();
	let mut lock_files = Vec::new();
	for root in legacy_roots {
		collect_application_files(root, &mut files, &mut lock_files)?;
	}
	if files.is_empty() {
		create_paths(&paths)?;
		Ok(Preparation::Ready)
	} else if paths_have_state(&paths)?
		|| (files.iter().any(|(_, disposition)| {
			matches!(disposition, LegacyFileDisposition::Root)
		}) && paths.root.join("config.toml").try_exists()?)
	{
		let legacy = files[0].0.parent().unwrap_or(&files[0].0);
		Err(io::Error::new(
			io::ErrorKind::AlreadyExists,
			format!(
				"Application state exists in both {} and {}",
				legacy.display(),
				paths.root.display()
			),
		))
	} else {
		Ok(Preparation::Migration(Migration {
			application_home: paths,
			legacy_roots: legacy_roots.to_vec(),
			files,
			lock_files,
			migration_lock: None,
		}))
	}
}

fn collect_application_files(
	root: &Path,
	files: &mut Vec<(PathBuf, LegacyFileDisposition)>,
	lock_files: &mut Vec<PathBuf>,
) -> io::Result<()> {
	let entries = match fs::read_dir(root) {
		Ok(entries) => entries,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
		Err(error) => return Err(error),
	};
	for entry in entries {
		let entry = entry?;
		let path = entry.path();
		let file_type = entry.file_type()?;
		if file_type.is_dir() {
			collect_application_files(&path, files, lock_files)?;
			continue;
		}
		let Some(disposition) = file_type
			.is_file()
			.then(|| application_file_disposition(&path))
			.flatten()
		else {
			return Err(io::Error::new(
				io::ErrorKind::InvalidData,
				format!(
					"Unrecognized legacy application file: {}",
					path.display()
				),
			));
		};
		match disposition {
			LegacyFileDisposition::Discard => lock_files.push(path),
			LegacyFileDisposition::Root
			| LegacyFileDisposition::Cache
			| LegacyFileDisposition::Data => {
				files.push((path, disposition));
			}
		}
	}
	Ok(())
}

fn create_paths(paths: &Paths) -> io::Result<()> {
	fs::create_dir_all(&paths.cache)?;
	fs::create_dir_all(&paths.data)?;
	private_directory(&paths.root)?;
	private_directory(&paths.cache)?;
	private_directory(&paths.data)
}

#[cfg(unix)]
fn private_directory(path: &Path) -> io::Result<()> {
	use std::os::unix::fs::PermissionsExt;

	fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_directory(_path: &Path) -> io::Result<()> {
	Ok(())
}

#[cfg(unix)]
pub(crate) fn private_file(path: &Path) -> io::Result<()> {
	use std::os::unix::fs::PermissionsExt;

	fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub(crate) fn private_file(_path: &Path) -> io::Result<()> {
	Ok(())
}

fn paths_have_state(paths: &Paths) -> io::Result<bool> {
	for directory in [&paths.cache, &paths.data] {
		let entries = match fs::read_dir(directory) {
			Ok(entries) => entries,
			Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
			Err(error) => return Err(error),
		};
		for entry in entries {
			if matches!(
				application_file_disposition(&entry?.path()),
				Some(
					LegacyFileDisposition::Cache | LegacyFileDisposition::Data
				)
			) {
				return Ok(true);
			}
		}
	}
	Ok(false)
}

fn application_file_disposition(path: &Path) -> Option<LegacyFileDisposition> {
	match path.file_name()?.to_str()? {
		"config.toml" => Some(LegacyFileDisposition::Root),
		"cache.sqlite"
		| "cache.sqlite-wal"
		| "cache.sqlite-shm"
		| "series.json"
		| "latest-release.json" => Some(LegacyFileDisposition::Cache),
		"telemetry.sqlite"
		| "telemetry.sqlite-wal"
		| "telemetry.sqlite-shm"
		| "telemetry.json"
		| "telemetry-disabled" => Some(LegacyFileDisposition::Data),
		"cache.lock"
		| "cache-initialization.lock"
		| "telemetry.lock"
		| "telemetry-initialization.lock" => Some(LegacyFileDisposition::Discard),
		_ => None,
	}
}

fn staging_paths(paths: &Paths) -> io::Result<Paths> {
	let name = paths
		.root
		.file_name()
		.and_then(|name| name.to_str())
		.ok_or_else(|| io::Error::other("application home has no file name"))?;
	Ok(Paths::at(
		paths.root.with_file_name(format!("{name}.staging")),
	))
}

fn prepare() -> io::Result<Option<Preparation>> {
	let Some(paths) = paths() else {
		return Ok(None);
	};
	let legacy_roots = legacy_roots(&paths);
	let migration_lock = migration::MigrationLock::acquire(&paths)?;
	let mut recovery_roots = legacy_roots.clone();
	recovery_roots.push(paths.root.clone());
	migration::recover_tombstones(&paths, &recovery_roots)?;
	let preparation = prepare_at(paths, &legacy_roots)?;
	Ok(Some(match preparation {
		Preparation::Ready => Preparation::Ready,
		Preparation::Migration(mut migration) => {
			migration.migration_lock = Some(migration_lock);
			Preparation::Migration(migration)
		}
	}))
}

fn paths() -> Option<Paths> {
	BaseDirs::new().map(|directories| {
		Paths::at(directories.home_dir().join(format!(".{APPLICATION_ID}")))
	})
}

pub(crate) fn cache_directory() -> Option<PathBuf> {
	paths().map(|paths| paths.cache)
}

pub(crate) fn application_home() -> Option<PathBuf> {
	paths().map(|paths| paths.root)
}

pub(crate) fn data_directory() -> Option<PathBuf> {
	paths().map(|paths| paths.data)
}

pub(crate) fn purge() -> io::Result<()> {
	let paths = paths();
	let _migration_lock = paths
		.as_ref()
		.map(migration::MigrationLock::acquire)
		.transpose()?;
	let mut roots = paths.as_ref().map_or_else(Vec::new, legacy_roots);
	let tombstones = roots
		.iter()
		.map(|root| migration::tombstone_path(root))
		.collect::<io::Result<Vec<_>>>()?;
	roots.extend(tombstones);
	if let Some(paths) = paths {
		roots.push(migration::tombstone_path(&paths.root)?);
		roots.push(staging_paths(&paths)?.root);
		roots.push(paths.root);
	}
	roots.sort_unstable();
	roots.dedup();
	purge_directories(&roots)
}

fn legacy_roots(paths: &Paths) -> Vec<PathBuf> {
	let mut roots = Vec::new();
	if let Some(base) = BaseDirs::new() {
		roots.push(base.home_dir().join(format!(".{LEGACY_APPLICATION_ID}")));
	}
	for application_id in [APPLICATION_ID, LEGACY_APPLICATION_ID] {
		if let Some(directories) = ProjectDirs::from("", "", application_id) {
			roots.extend(application_roots(&directories));
		}
	}
	roots.retain(|root| root != &paths.root);
	roots.sort_unstable();
	roots.dedup();
	roots
}

fn application_roots(directories: &ProjectDirs) -> Vec<PathBuf> {
	let mut paths = vec![
		directories.cache_dir(),
		directories.config_dir(),
		directories.config_local_dir(),
		directories.data_dir(),
		directories.data_local_dir(),
		directories.preference_dir(),
	];
	paths.extend(directories.runtime_dir());
	paths.extend(directories.state_dir());

	let project_path = directories.project_path();
	let mut roots = paths
		.into_iter()
		.map(|path| {
			path.ancestors()
				.find(|ancestor| ancestor.ends_with(project_path))
				.unwrap_or(path)
				.to_owned()
		})
		.collect::<Vec<_>>();
	roots.sort_unstable();
	roots.dedup();
	roots
}

fn purge_directories(directories: &[PathBuf]) -> io::Result<()> {
	let failures = directories
		.iter()
		.filter_map(|directory| match fs::remove_dir_all(directory) {
			Ok(()) => None,
			Err(error) if error.kind() == io::ErrorKind::NotFound => None,
			Err(error) => Some(format!("{}: {error}", directory.display())),
		})
		.collect::<Vec<_>>();
	if failures.is_empty() {
		Ok(())
	} else {
		Err(io::Error::other(failures.join("\n")))
	}
}

#[cfg(test)]
#[path = "app_files_tests.rs"]
mod tests;
