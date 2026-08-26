use std::{
	fs::{self, OpenOptions},
	process,
	time::SystemTime,
};

use pretty_assertions::assert_eq;

use super::{Migration, Paths, Preparation, prepare_at, purge_directories};

#[test]
fn stages_and_commits_legacy_application_state() {
	let fixture = Fixture::new("migration");
	let nested_cache = fixture.legacy.join("cache");
	fs::create_dir(&nested_cache).unwrap();
	fs::rename(
		fixture.legacy.join("cache.sqlite"),
		nested_cache.join("cache.sqlite"),
	)
	.unwrap();
	let legacy_data = fixture.base.join("legacy-data");
	fs::create_dir_all(&legacy_data).unwrap();
	fs::write(legacy_data.join("telemetry.sqlite"), b"telemetry").unwrap();
	let Preparation::Migration(migration) = prepare_at(
		fixture.paths.clone(),
		&[fixture.legacy.clone(), legacy_data.clone()],
	)
	.unwrap() else {
		panic!("expected migration");
	};
	let staged = migration.lock().unwrap().stage().unwrap();
	assert_eq!(
		(
			fs::read(staged.paths().cache.join("cache.sqlite")).unwrap(),
			fs::read(staged.paths().data.join("telemetry.sqlite")).unwrap(),
		),
		(b"cache".to_vec(), b"telemetry".to_vec()),
	);
	let staging_root = staged.paths().root.clone();
	fs::remove_dir_all(&staging_root).unwrap();
	assert_eq!(
		staged.commit().unwrap_err().kind(),
		std::io::ErrorKind::NotFound,
	);
	assert!(fixture.legacy.exists());
	assert!(
		!super::migration::tombstone_path(&fixture.legacy)
			.unwrap()
			.exists()
	);
	let Preparation::Migration(migration) = prepare_at(
		fixture.paths.clone(),
		&[fixture.legacy.clone(), legacy_data.clone()],
	)
	.unwrap() else {
		panic!("expected migration");
	};
	let staged = migration.lock().unwrap().stage().unwrap();

	assert_eq!(staged.commit().unwrap(), fixture.paths);
	assert_eq!(
		(fixture.legacy.exists(), legacy_data.exists()),
		(false, false),
	);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;

		let mode =
			|path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
		assert_eq!(
			[
				mode(&fixture.paths.root),
				mode(&fixture.paths.cache),
				mode(&fixture.paths.data),
				mode(&fixture.paths.cache.join("cache.sqlite")),
			],
			[0o700, 0o700, 0o700, 0o600],
		);
	}
}

#[test]
fn preserves_configuration_during_legacy_application_state_migration() {
	let fixture = Fixture::new("configuration");
	fs::create_dir_all(&fixture.paths.root).unwrap();
	fs::write(fixture.paths.root.join("config.toml"), b"jobs = 8\n").unwrap();

	fixture
		.migration()
		.lock()
		.unwrap()
		.stage()
		.unwrap()
		.commit()
		.unwrap();

	assert_eq!(
		fs::read(fixture.paths.root.join("config.toml")).unwrap(),
		b"jobs = 8\n"
	);
}

#[test]
fn migrates_configuration_from_the_renamed_application_home() {
	let fixture = Fixture::new("renamed-configuration");
	fs::write(fixture.legacy.join("config.toml"), b"adult = true\n").unwrap();

	fixture
		.migration()
		.lock()
		.unwrap()
		.stage()
		.unwrap()
		.commit()
		.unwrap();

	assert_eq!(
		fs::read(fixture.paths.root.join("config.toml")).unwrap(),
		b"adult = true\n"
	);
}

#[test]
fn new_only_application_state_is_left_unchanged() {
	let fixture = Fixture::new("new-only");
	fs::remove_dir_all(&fixture.legacy).unwrap();
	fs::create_dir_all(&fixture.paths.cache).unwrap();
	fs::write(fixture.paths.cache.join("cache.sqlite"), b"current").unwrap();

	let preparation = prepare_at(
		fixture.paths.clone(),
		std::slice::from_ref(&fixture.legacy),
	)
	.unwrap();

	assert!(matches!(preparation, Preparation::Ready));
	assert_eq!(
		fs::read(fixture.paths.cache.join("cache.sqlite")).unwrap(),
		b"current"
	);
}

#[test]
fn completed_application_state_migration_is_idempotent() {
	let fixture = Fixture::new("repeated");
	fixture
		.migration()
		.lock()
		.unwrap()
		.stage()
		.unwrap()
		.commit()
		.unwrap();

	let repeated = prepare_at(
		fixture.paths.clone(),
		std::slice::from_ref(&fixture.legacy),
	)
	.unwrap();

	assert!(matches!(repeated, Preparation::Ready));
	assert_eq!(
		fs::read(fixture.paths.cache.join("cache.sqlite")).unwrap(),
		b"cache"
	);
}

#[test]
fn interrupted_tombstones_restore_or_discard_according_to_commit_state() {
	let restoring = Fixture::new("restore-tombstone");
	let restoring_tombstone =
		super::migration::tombstone_path(&restoring.legacy).unwrap();
	fs::rename(&restoring.legacy, &restoring_tombstone).unwrap();
	super::migration::recover_tombstones(
		&restoring.paths,
		std::slice::from_ref(&restoring.legacy),
	)
	.unwrap();

	let committed = Fixture::new("discard-tombstone");
	fs::create_dir_all(&committed.paths.root).unwrap();
	let committed_tombstone =
		super::migration::tombstone_path(&committed.legacy).unwrap();
	fs::rename(&committed.legacy, &committed_tombstone).unwrap();
	super::migration::recover_tombstones(
		&committed.paths,
		std::slice::from_ref(&committed.legacy),
	)
	.unwrap();

	assert_eq!(
		(
			restoring.legacy.exists(),
			restoring_tombstone.exists(),
			committed.legacy.exists(),
			committed_tombstone.exists(),
		),
		(true, false, false, false),
	);
}

#[test]
fn rejects_unrecognized_legacy_files_without_creating_the_home() {
	let fixture = Fixture::new("unknown");
	let unknown = fixture.legacy.join("mystery.db");
	fs::write(&unknown, b"unknown").unwrap();

	let error = preparation_error(&fixture);

	assert_eq!(
		error.to_string(),
		format!(
			"Unrecognized legacy application file: {}",
			unknown.display()
		),
	);
	assert!(!fixture.paths.root.exists());
}

#[test]
fn rejects_conflicting_legacy_and_application_home_state() {
	let fixture = Fixture::new("conflict");
	fs::create_dir_all(&fixture.paths.cache).unwrap();
	fs::write(fixture.paths.cache.join("cache.sqlite"), b"current").unwrap();

	let error = preparation_error(&fixture);

	assert_eq!(
		error.to_string(),
		format!(
			"Application state exists in both {} and {}",
			fixture.legacy.display(),
			fixture.paths.root.display()
		),
	);
	assert_eq!(
		fs::read(fixture.paths.cache.join("cache.sqlite")).unwrap(),
		b"current",
	);
}

#[test]
fn active_legacy_state_blocks_migration() {
	for lock_name in ["cache.lock", "telemetry.lock"] {
		let fixture = Fixture::new(lock_name);
		let lock_directory = fixture.legacy.join("nested");
		fs::create_dir(&lock_directory).unwrap();
		let lock = OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.open(lock_directory.join(lock_name))
			.unwrap();
		lock.try_lock_shared().unwrap();

		let error = match fixture.migration().lock() {
			Err(error) => error,
			Ok(_) => panic!("expected an active-state error"),
		};

		assert_eq!(
			error.to_string(),
			"Legacy application state is in use; close other a365 or a365dt processes and retry."
		);
		assert_eq!(fixture.cache(), b"cache");
		assert!(!fixture.paths.root.exists());
	}
}

#[test]
fn serializes_application_file_migrations() {
	let fixture = Fixture::new("migration-lock");
	let migration_lock =
		super::migration::MigrationLock::acquire(&fixture.paths).unwrap();

	let error = super::migration::MigrationLock::acquire(&fixture.paths)
		.err()
		.expect("the second migration must be blocked");

	assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
	drop(migration_lock);
	assert!(
		!super::migration::lock_path(&fixture.paths)
			.unwrap()
			.exists()
	);
}

#[test]
fn purges_every_owned_directory_idempotently() {
	let base = temporary_directory("purge");
	let directories = [base.join("cache"), base.join("data")];
	for directory in &directories {
		fs::create_dir_all(directory.join("nested")).unwrap();
		fs::write(directory.join("nested/file"), b"owned").unwrap();
	}

	purge_directories(&directories).unwrap();
	purge_directories(&directories).unwrap();

	assert_eq!(directories.map(|directory| directory.exists()), [false; 2]);
	fs::remove_dir(base).unwrap();
}

struct Fixture {
	base: std::path::PathBuf,
	legacy: std::path::PathBuf,
	paths: Paths,
}

impl Fixture {
	fn new(name: &str) -> Self {
		let base = temporary_directory(name);
		let legacy = base.join("legacy");
		fs::create_dir_all(&legacy).unwrap();
		fs::write(legacy.join("cache.sqlite"), b"cache").unwrap();
		let paths = Paths::at(base.join(".a365"));
		Self {
			base,
			legacy,
			paths,
		}
	}

	fn migration(&self) -> Migration {
		let Preparation::Migration(migration) =
			prepare_at(self.paths.clone(), std::slice::from_ref(&self.legacy))
				.unwrap()
		else {
			panic!("expected migration");
		};
		migration
	}

	fn cache(&self) -> Vec<u8> {
		fs::read(self.legacy.join("cache.sqlite")).unwrap()
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		fs::remove_dir_all(&self.base).unwrap();
	}
}

fn preparation_error(fixture: &Fixture) -> std::io::Error {
	match prepare_at(
		fixture.paths.clone(),
		std::slice::from_ref(&fixture.legacy),
	) {
		Err(error) => error,
		Ok(_) => panic!("expected preparation to fail"),
	}
}

fn temporary_directory(name: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!(
		"a365dt-app-files-{name}-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	))
}
