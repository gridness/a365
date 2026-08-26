use std::{
	fs::{self, File, OpenOptions},
	io,
	path::{Path, PathBuf},
};

use crate::{
	cache::{self, MigrationPreparation as CachePreparation},
	error::Error,
	telemetry::{
		self, MigrationPreparation as TelemetryPreparation, TelemetryRecovery,
	},
	ui,
};

use super::{Paths, Preparation, private_file, purge_directories};

pub(super) struct MigrationLock {
	_file: File,
	path: PathBuf,
}

impl MigrationLock {
	pub(super) fn acquire(paths: &Paths) -> io::Result<Self> {
		let path = lock_path(paths)?;
		let file = OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.open(&path)?;
		file.try_lock().map_err(|error| match error {
			fs::TryLockError::WouldBlock => io::Error::new(
				io::ErrorKind::WouldBlock,
				"Another application-file migration is active.",
			),
			fs::TryLockError::Error(error) => error,
		})?;
		private_file(&path)?;
		Ok(Self { _file: file, path })
	}
}

impl Drop for MigrationLock {
	fn drop(&mut self) {
		let _ = fs::remove_file(&self.path);
	}
}

pub(super) fn lock_path(paths: &Paths) -> io::Result<PathBuf> {
	let name = paths
		.root
		.file_name()
		.and_then(|name| name.to_str())
		.ok_or_else(|| io::Error::other("application home has no file name"))?;
	Ok(paths.root.with_file_name(format!("{name}.migration.lock")))
}

pub(crate) async fn prepare_for_command() -> Result<(), Error> {
	let preparation = super::prepare()
		.map_err(|error| {
			file_error("Could not prepare application files.", error)
		})?
		.ok_or_else(|| {
			Error::new("Could not resolve the user home directory.")
		})?;
	let Preparation::Migration(migration) = preparation else {
		return Ok(());
	};
	if ui::selector::interactive_terminal() {
		ui::heading("Application file migration");
		ui::note("a365 is moving existing application files to ~/.a365.");
		ui::grid(&migration.moves());
	}
	let telemetry_database = migration
		.legacy_telemetry_database()
		.map(std::path::Path::to_owned);
	let migration = migration.lock().map_err(|error| {
		file_error("Could not lock the legacy application files.", error)
	})?;
	let telemetry_lock = match telemetry_database {
		Some(database) => telemetry::ensure_migration_idle(&database)?,
		None => None,
	};
	let staged = migration.stage().map_err(|error| {
		file_error("Could not stage the application file migration.", error)
	})?;
	if cache::prepare_migration_at(&staged.paths().cache).await?
		== CachePreparation::Rebuilt
	{
		ui::note("Rebuilt damaged cache.");
	}
	if telemetry::prepare_migration_at(&staged.paths().data).await?
		== TelemetryPreparation::Damaged
	{
		ui::warning(
			"Telemetry history is damaged and cannot be migrated. Continuing will discard it.",
		);
		let rows = [
			["Recreate telemetry enabled".to_owned()],
			["Recreate telemetry disabled".to_owned()],
			["Cancel migration".to_owned()],
		];
		let recoveries = [
			Some(TelemetryRecovery::Enabled),
			Some(TelemetryRecovery::Disabled),
			None,
		];
		let Some(recovery) =
			recoveries[ui::choose("How should a365 proceed?", &rows)?]
		else {
			return Err(Error::new("Application file migration cancelled."));
		};
		telemetry::recreate_migration_at(&staged.paths().data, recovery)
			.await?;
	}
	let committed = staged.commit().map_err(|error| {
		file_error("Could not finish the application file migration.", error)
	});
	if let Some(telemetry_lock) = telemetry_lock {
		telemetry_lock.close()?;
	}
	committed?;
	ui::success("Application files moved");
	Ok(())
}

fn file_error(message: &str, error: std::io::Error) -> Error {
	Error::with_debug(format!("{message} {error}"), error)
}

pub(super) fn tombstone_path(root: &Path) -> io::Result<PathBuf> {
	let name =
		root.file_name()
			.and_then(|name| name.to_str())
			.ok_or_else(|| {
				io::Error::other("legacy application root has no file name")
			})?;
	Ok(root.with_file_name(format!("{name}.migrating")))
}

pub(super) fn move_to_tombstones(
	roots: &[PathBuf],
) -> io::Result<Vec<(PathBuf, PathBuf)>> {
	let tombstones = roots
		.iter()
		.map(|root| Ok((root.clone(), tombstone_path(root)?)))
		.collect::<io::Result<Vec<_>>>()?;
	let mut moved = Vec::new();
	for (root, tombstone) in tombstones {
		match fs::rename(&root, &tombstone) {
			Ok(()) => moved.push((root, tombstone)),
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				let _ = restore_tombstones(&moved);
				return Err(error);
			}
		}
	}
	Ok(moved)
}

pub(super) fn restore_tombstones(
	tombstones: &[(PathBuf, PathBuf)],
) -> io::Result<()> {
	let failures = tombstones
		.iter()
		.rev()
		.filter_map(|(root, tombstone)| fs::rename(tombstone, root).err())
		.map(|error| error.to_string())
		.collect::<Vec<_>>();
	if failures.is_empty() {
		Ok(())
	} else {
		Err(io::Error::other(failures.join("\n")))
	}
}

pub(super) fn recover_tombstones(
	paths: &Paths,
	roots: &[PathBuf],
) -> io::Result<()> {
	let tombstones = roots
		.iter()
		.map(|root| Ok((root.clone(), tombstone_path(root)?)))
		.collect::<io::Result<Vec<_>>>()?;
	if paths.root.exists() {
		purge_directories(
			&tombstones
				.iter()
				.map(|(_, tombstone)| tombstone.clone())
				.collect::<Vec<_>>(),
		)
	} else {
		restore_tombstones(
			&tombstones
				.into_iter()
				.filter(|(_, tombstone)| tombstone.exists())
				.collect::<Vec<_>>(),
		)
	}
}
