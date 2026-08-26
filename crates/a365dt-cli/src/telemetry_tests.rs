use std::{
	collections::BTreeMap,
	fs,
	time::{Duration, SystemTime},
};

use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use uuid::{Uuid, Version};

use super::{
	CatalogueUse, Command, CommandOutcome, InvocationId, MigrationPreparation,
	Operation, Paths, PlaybackOutcome, Recorder, SeriesRecording,
	TelemetryRecovery, Writer,
	display::format_timestamp,
	ensure_migration_idle, prepare_migration_at,
	recording::{
		DownloadOutcome, Observation, ObservationKind, SeriesIdentity,
	},
	recreate_migration_at, snapshot,
	storage::Store,
};
use crate::{
	api::{Episode, Series},
	content::ContentSource,
	download::{Outcome, Status, Summary},
	error::Error,
};

#[test]
fn recorder_sends_complete_typed_privacy_safe_observations_from_clones() {
	let invocation_id = InvocationId::new();
	let (observations, mut receiver) = mpsc::unbounded_channel();
	let recorder = Recorder::connected(invocation_id, observations);
	recorder.record_command(Command::Download, CommandOutcome::Success);
	recorder.clone().record_series(
		&series(),
		CatalogueUse::Miss,
		SeriesRecording::IncludeIdentity,
	);
	recorder.record_download(
		&series(),
		&Summary {
			outcomes: vec![
				Outcome {
					episode: "secret episode".into(),
					status: Status::Downloaded,
					bytes: 42,
					detail: Error::new("secret path"),
				},
				Outcome {
					episode: "secret existing episode".into(),
					status: Status::Skipped,
					bytes: 100,
					detail: Error::new("secret existing path"),
				},
			],
			elapsed: Duration::from_micros(12_345),
		},
		SeriesRecording::IncludeIdentity,
	);
	drop(recorder.measure_items(Operation::SearchRank, 30_000));
	drop(recorder);
	let mut observations =
		std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();

	assert!(
		observations
			.iter()
			.all(|observation| observation.invocation_id == invocation_id)
	);
	let performance = observations.pop().unwrap().kind;
	assert_eq!(
		observations
			.into_iter()
			.map(|observation| observation.kind)
			.collect::<Vec<_>>(),
		vec![
			ObservationKind::Command {
				command: Command::Download,
				outcome: CommandOutcome::Success,
			},
			ObservationKind::SeriesSelection {
				identity: identity(),
				catalogue: Some(CatalogueUse::Miss),
			},
			ObservationKind::DownloadBatch {
				identity: identity(),
				duration_us: 12_345,
				outcomes: vec![
					DownloadOutcome {
						status: Status::Downloaded,
						bytes: Some(42),
					},
					DownloadOutcome {
						status: Status::Skipped,
						bytes: None,
					},
				],
			},
		]
	);
	assert!(matches!(
		performance,
		ObservationKind::Performance {
			operation: Operation::SearchRank,
			duration_us: _,
			work_units: Some(30_000),
		}
	));
	let parsed = Uuid::parse_str(&invocation_id.to_string()).unwrap();
	assert_eq!(
		(parsed.get_version(), invocation_id.to_string().len()),
		(Some(Version::SortRand), 36)
	);
}

#[test]
fn aggregate_only_adult_observations_exclude_source_id_and_title() {
	let invocation_id = InvocationId::new();
	let (observations, mut receiver) = mpsc::unbounded_channel();
	let recorder = Recorder::connected(invocation_id, observations);
	let mut adult = series();
	adult.source = ContentSource::H365;
	adult.episodes[0].source = ContentSource::H365;
	recorder.record_series(
		&adult,
		CatalogueUse::Bypassed,
		SeriesRecording::AggregateOnly,
	);
	recorder.record_playback(
		&adult,
		Duration::from_secs(1),
		PlaybackOutcome::NaturalEnd,
		SeriesRecording::AggregateOnly,
	);
	drop(recorder);

	assert_eq!(
		std::iter::from_fn(|| receiver.try_recv().ok())
			.map(|observation| observation.kind)
			.collect::<Vec<_>>(),
		vec![
			ObservationKind::SeriesSelection {
				identity: SeriesIdentity::AggregateOnly,
				catalogue: None,
			},
			ObservationKind::Playback {
				identity: SeriesIdentity::AggregateOnly,
				duration_us: 1_000_000,
				outcome: PlaybackOutcome::NaturalEnd,
			},
		]
	);
}

fn identity() -> SeriesIdentity {
	SeriesIdentity::Included {
		source: ContentSource::Anime365,
		id: 365,
		title: "Private Series title".into(),
	}
}

#[tokio::test]
async fn batch_commit_rechecks_collection_state_and_resumes_after_reenable() {
	let paths = paths("state-rechecks");
	let control = Store::open(paths.clone()).await.unwrap();
	let mut watermark =
		control.collection_state().await.unwrap().last_cleared_at_ms;
	let invocation_id = InvocationId::new();
	control.disable(InvocationId::new()).await.unwrap();
	control
		.commit(
			&mut watermark,
			vec![command_observation(invocation_id, 1, Command::Download)],
		)
		.await
		.unwrap();
	control.enable(InvocationId::new()).await.unwrap();
	control
		.commit(
			&mut watermark,
			vec![command_observation(invocation_id, 2, Command::Update)],
		)
		.await
		.unwrap();

	assert_eq!(
		snapshot::capture(&control).await.unwrap().counters,
		BTreeMap::from([
			("commands.telemetry.disable.success".into(), 1),
			("commands.telemetry.enable.success".into(), 1),
			("commands.update.success".into(), 1),
		])
	);
	control.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn disabled_at_start_recorder_stays_disabled_after_reenable() {
	let paths = paths("disabled-start");
	let control = Store::open(paths.clone()).await.unwrap();
	control.disable(InvocationId::new()).await.unwrap();
	let (recorder, writer) = Writer::at(paths.clone(), InvocationId::new())
		.await
		.unwrap();
	control.enable(InvocationId::new()).await.unwrap();
	recorder.record_command(Command::Download, CommandOutcome::Success);
	writer.finish().await.unwrap();

	assert_eq!(
		snapshot::capture(&control).await.unwrap().counters,
		BTreeMap::from([
			("commands.telemetry.disable.success".into(), 1),
			("commands.telemetry.enable.success".into(), 1),
		])
	);
	control.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn clear_watermark_discards_buffered_history_atomically() {
	let paths = paths("watermark");
	let store = Store::open(paths.clone()).await.unwrap();
	let mut baseline =
		store.collection_state().await.unwrap().last_cleared_at_ms;
	store
		.clear(super::storage::ClearRange::All, super::recording::now_ms())
		.await
		.unwrap();
	let watermark = store
		.collection_state()
		.await
		.unwrap()
		.last_cleared_at_ms
		.unwrap();
	let invocation_id = InvocationId::new();
	store
		.commit(
			&mut baseline,
			vec![
				command_observation(
					invocation_id,
					watermark,
					Command::Download,
				),
				command_observation(
					invocation_id,
					watermark + 1,
					Command::Update,
				),
			],
		)
		.await
		.unwrap();

	assert_eq!(
		snapshot::capture(&store).await.unwrap().counters,
		BTreeMap::from([("commands.update.success".into(), 1)])
	);
	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn preserves_legacy_opt_out_before_retiring_aggregate_files() {
	let paths = paths("cutover");
	fs::create_dir_all(paths.data.parent().unwrap()).unwrap();
	fs::create_dir_all(paths.disabled.parent().unwrap()).unwrap();
	fs::write(paths.data.with_file_name("telemetry.json"), b"legacy").unwrap();
	fs::write(&paths.lock, b"legacy").unwrap();
	fs::write(&paths.disabled, b"123").unwrap();

	let store = Store::open(paths.clone()).await.unwrap();

	assert_eq!(
		store.collection_state().await.unwrap(),
		super::storage::CollectionState {
			enabled: false,
			last_enabled_at_ms: None,
			last_disabled_at_ms: Some(123_000),
			last_cleared_at_ms: None,
		}
	);
	assert!(!paths.data.with_file_name("telemetry.json").exists());
	assert!(!paths.lock.exists());
	assert!(!paths.disabled.exists());
	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn failed_initialization_keeps_every_legacy_file() {
	let paths = paths("failed-cutover");
	fs::create_dir_all(&paths.data).unwrap();
	fs::create_dir_all(paths.disabled.parent().unwrap()).unwrap();
	let legacy = paths.data.with_file_name("telemetry.json");
	fs::write(&legacy, b"legacy").unwrap();
	fs::write(&paths.lock, b"legacy").unwrap();
	fs::write(&paths.disabled, b"123").unwrap();

	let error = match Store::open(paths.clone()).await {
		Ok(_) => panic!("invalid telemetry path should fail"),
		Err(error) => error,
	};

	assert!(
		error
			.to_string()
			.contains("Could not open the local telemetry")
	);
	assert_eq!(
		(
			fs::read(legacy).unwrap(),
			fs::read(&paths.lock).unwrap(),
			fs::read(&paths.disabled).unwrap(),
		),
		(b"legacy".to_vec(), b"legacy".to_vec(), b"123".to_vec())
	);
	cleanup(&paths);
}

#[tokio::test]
async fn invalid_legacy_opt_out_keeps_every_legacy_file() {
	let paths = paths("invalid-opt-out");
	fs::create_dir_all(paths.data.parent().unwrap()).unwrap();
	fs::create_dir_all(paths.disabled.parent().unwrap()).unwrap();
	let legacy = paths.data.with_file_name("telemetry.json");
	fs::write(&legacy, b"legacy").unwrap();
	fs::write(&paths.lock, b"legacy").unwrap();
	fs::write(&paths.disabled, b"invalid").unwrap();

	let error = match Store::open(paths.clone()).await {
		Ok(_) => panic!("invalid opt-out timestamp should fail"),
		Err(error) => error,
	};

	assert!(
		error
			.to_string()
			.contains("Could not open the local telemetry")
	);
	assert!(!paths.data.exists());
	assert_eq!(
		(
			fs::read(legacy).unwrap(),
			fs::read(&paths.lock).unwrap(),
			fs::read(&paths.disabled).unwrap(),
		),
		(b"legacy".to_vec(), b"legacy".to_vec(), b"invalid".to_vec())
	);
	cleanup(&paths);
}

#[tokio::test]
async fn recreates_damaged_migration_telemetry_with_the_chosen_state() {
	let mut enabled = Vec::new();
	for (name, recovery) in [
		("recreate-enabled", TelemetryRecovery::Enabled),
		("recreate-disabled", TelemetryRecovery::Disabled),
	] {
		let paths = paths(name);
		let directory = paths.data.parent().unwrap();
		fs::create_dir_all(directory).unwrap();
		fs::write(&paths.data, b"damaged").unwrap();

		recreate_migration_at(directory, recovery).await.unwrap();
		let store = Store::open(Paths::at(directory.to_owned())).await.unwrap();
		enabled.push(store.collection_state().await.unwrap().enabled);
		store.close().await;
		cleanup(&paths);
	}

	assert_eq!(enabled, [true, false]);

	let paths = paths("recreate-delete-failure");
	fs::create_dir_all(&paths.data).unwrap();
	let error = recreate_migration_at(
		paths.data.parent().unwrap(),
		TelemetryRecovery::Enabled,
	)
	.await
	.unwrap_err();
	assert!(error.to_string().contains("Could not recreate"));
	cleanup(&paths);
}

#[tokio::test]
async fn migration_validation_distinguishes_damage_from_operational_failures() {
	let migration_paths = paths("migration-validation");
	let directory = migration_paths.data.parent().unwrap();
	fs::create_dir_all(directory).unwrap();
	fs::write(&migration_paths.data, b"damaged").unwrap();
	assert_eq!(
		prepare_migration_at(directory).await.unwrap(),
		MigrationPreparation::Damaged,
	);
	cleanup(&migration_paths);

	let migration_paths = paths("migration-overflowing-opt-out");
	let directory = migration_paths.data.parent().unwrap();
	fs::create_dir_all(directory).unwrap();
	fs::write(directory.join("telemetry-disabled"), u64::MAX.to_string())
		.unwrap();
	assert_eq!(
		prepare_migration_at(directory).await.unwrap(),
		MigrationPreparation::Damaged,
	);
	cleanup(&migration_paths);

	let migration_paths = paths("migration-operational-error");
	let directory = migration_paths.data.parent().unwrap();
	fs::create_dir_all(directory.parent().unwrap()).unwrap();
	fs::write(directory, b"not a directory").unwrap();
	assert!(prepare_migration_at(directory).await.is_err());
	cleanup(&migration_paths);
}

#[tokio::test]
async fn migration_lock_preserves_inactive_legacy_telemetry() {
	let paths = paths("active-home-migration");
	let store = Store::open(paths.clone()).await.unwrap();
	store.close().await;
	let files = crate::sqlite::files(&paths.data);
	fs::write(&files[1], b"wal").unwrap();
	fs::write(&files[2], b"shm").unwrap();
	let before = files.each_ref().map(|path| fs::read(path).unwrap());
	let migration_lock = ensure_migration_idle(&paths.data).unwrap().unwrap();
	migration_lock.close().unwrap();
	assert_eq!(files.each_ref().map(|path| fs::read(path).unwrap()), before,);
	cleanup(&paths);
}

#[test]
fn formats_utc_calendar_dates() {
	assert_eq!(
		[
			format_timestamp(None),
			format_timestamp(Some(0)),
			format_timestamp(Some(951_782_400)),
		],
		[
			"Never",
			"1970-01-01 00:00:00 UTC",
			"2000-02-29 00:00:00 UTC",
		]
	);
}

fn command_observation(
	invocation_id: InvocationId,
	observed_at_ms: u64,
	command: Command,
) -> Observation {
	Observation {
		invocation_id,
		observed_at_ms,
		kind: ObservationKind::Command {
			command,
			outcome: CommandOutcome::Success,
		},
	}
}

fn paths(name: &str) -> Paths {
	let root = std::env::temp_dir().join(format!(
		"a365-telemetry-{name}-{}-{}",
		std::process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	Paths {
		data: root.join("data/telemetry.sqlite"),
		lock: root.join("data/telemetry.lock"),
		disabled: root.join("config/telemetry-disabled"),
	}
}

fn cleanup(paths: &Paths) {
	fs::remove_dir_all(paths.data.parent().unwrap().parent().unwrap()).unwrap();
}

fn series() -> Series {
	Series {
		source: crate::content::ContentSource::Anime365,
		id: 365,
		title: "Private Series title".into(),
		year: Some(2024),
		type_title: Some("TV".into()),
		number_of_episodes: Some(12),
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: Some("secret poster".into()),
		episodes: vec![Episode {
			source: crate::content::ContentSource::Anime365,
			id: 1,
			episode_int: "secret number".into(),
			episode_full: "secret episode".into(),
		}],
	}
}
