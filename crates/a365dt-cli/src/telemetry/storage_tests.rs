use std::{fs, process, time::SystemTime};

use pretty_assertions::assert_eq;

use super::{ClearRange, CollectionState, Store};
use crate::{
	content::ContentSource,
	download::Status,
	telemetry::{
		CatalogueUse, Command, CommandOutcome, InvocationId, Operation, Paths,
		PlaybackOutcome,
		recording::{
			DownloadOutcome, Observation, ObservationKind, SeriesIdentity,
		},
	},
};

#[tokio::test]
async fn stores_redacted_adult_activity_as_source_agnostic_aggregates() {
	let paths = paths("redacted-adult");
	let store = Store::open(paths.clone()).await.unwrap();
	let invocation_id = InvocationId::new();
	let mut watermark = None;
	store
		.commit(
			&mut watermark,
			vec![
				Observation {
					invocation_id,
					observed_at_ms: 1,
					kind: ObservationKind::SeriesSelection {
						identity: SeriesIdentity::AggregateOnly,
						catalogue: None,
					},
				},
				Observation {
					invocation_id,
					observed_at_ms: 2,
					kind: ObservationKind::Playback {
						identity: SeriesIdentity::AggregateOnly,
						duration_us: 3,
						outcome: PlaybackOutcome::NaturalEnd,
					},
				},
			],
		)
		.await
		.unwrap();

	assert_eq!(
		(
			sqlx::query_as::<
				_,
				(Option<String>, Option<i64>, Option<String>, bool),
			>(
				"SELECT series_source, series_id, series_title, \
				 identity_redacted FROM series_selection_events",
			)
			.fetch_one(&store.pool)
			.await
			.unwrap(),
			sqlx::query_as::<
				_,
				(Option<String>, Option<i64>, Option<String>, bool, String),
			>(
				"SELECT series_source, series_id, series_title, \
				 identity_redacted, outcome FROM playback_sessions",
			)
			.fetch_one(&store.pool)
			.await
			.unwrap(),
		),
		(
			(None, None, None, true),
			(None, None, None, true, "natural_end".into()),
		)
	);

	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn opens_the_durable_typed_telemetry_store() {
	let paths = paths("settings");
	let store = Store::open(paths.clone()).await.unwrap();

	assert_eq!(
		(
			sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
				.fetch_one(&store.pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
				.fetch_one(&store.pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
				.fetch_one(&store.pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
				.fetch_one(&store.pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>(
				"SELECT COUNT(*) FROM pragma_table_list \
				 WHERE name IN (\
				 'collection_state', 'command_events', \
				 'series_selection_events', 'download_batches', \
				 'download_outcomes', 'playback_sessions', \
				 'performance_events'\
				 ) AND strict = 1",
			)
			.fetch_one(&store.pool)
			.await
			.unwrap(),
		),
		("wal".into(), 2, 1, 5_000, 7)
	);

	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn stores_complete_typed_observations() {
	let paths = paths("events");
	let store = Store::open(paths.clone()).await.unwrap();
	let invocation_id = InvocationId::new();
	let invocation = invocation_id.to_string();
	let mut watermark = None;
	store
		.commit(
			&mut watermark,
			vec![
				Observation {
					invocation_id,
					observed_at_ms: 10_000,
					kind: ObservationKind::Command {
						command: Command::Download,
						outcome: CommandOutcome::Success,
					},
				},
				Observation {
					invocation_id,
					observed_at_ms: 20_000,
					kind: ObservationKind::SeriesSelection {
						identity: identity(),
						catalogue: Some(CatalogueUse::Miss),
					},
				},
				Observation {
					invocation_id,
					observed_at_ms: 30_000,
					kind: ObservationKind::DownloadBatch {
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
				},
				Observation {
					invocation_id,
					observed_at_ms: 40_000,
					kind: ObservationKind::Performance {
						operation: Operation::SearchRank,
						duration_us: 50,
						work_units: Some(30_000),
					},
				},
			],
		)
		.await
		.unwrap();

	assert_eq!(
		(
			sqlx::query_as::<_, (String, i64, String, String)>(
				"SELECT invocation_id, observed_at_ms, command, outcome \
				 FROM command_events",
			)
			.fetch_all(&store.pool)
			.await
			.unwrap(),
			sqlx::query_as::<
				_,
				(String, i64, String, i64, String, bool, Option<String>),
			>(
				"SELECT invocation_id, observed_at_ms, series_source, \
				 series_id, series_title, identity_redacted, catalogue_result \
				 FROM series_selection_events",
			)
			.fetch_all(&store.pool)
			.await
			.unwrap(),
			sqlx::query_as::<_, (String, i64, String, i64, String, bool, i64)>(
				"SELECT invocation_id, observed_at_ms, series_source, \
				 series_id, series_title, identity_redacted, duration_us \
				 FROM download_batches",
			)
			.fetch_all(&store.pool)
			.await
			.unwrap(),
			sqlx::query_as::<_, (String, Option<i64>)>(
				"SELECT status, downloaded_bytes FROM download_outcomes \
				 ORDER BY id",
			)
			.fetch_all(&store.pool)
			.await
			.unwrap(),
			sqlx::query_as::<_, (String, i64, String, i64, Option<i64>)>(
				"SELECT invocation_id, observed_at_ms, operation, duration_us, \
				 work_units FROM performance_events",
			)
			.fetch_all(&store.pool)
			.await
			.unwrap(),
		),
		(
			vec![(
				invocation.clone(),
				10_000,
				"download".into(),
				"success".into(),
			)],
			vec![(
				invocation.clone(),
				20_000,
				"anime365".into(),
				365,
				"Private Series title".into(),
				false,
				Some("miss".into()),
			)],
			vec![(
				invocation.clone(),
				30_000,
				"anime365".into(),
				365,
				"Private Series title".into(),
				false,
				12_345,
			)],
			vec![("downloaded".into(), Some(42)), ("skipped".into(), None),],
			vec![(invocation, 40_000, "search.rank".into(), 50, Some(30_000),)],
		)
	);
	let snapshot = crate::telemetry::snapshot::capture(&store).await.unwrap();
	assert_eq!(
		(
			snapshot.first_recorded_at,
			snapshot.last_recorded_at,
			snapshot.first_download_at,
			snapshot.last_download_at,
			snapshot.counters,
			snapshot.samples,
			snapshot
				.performance
				.into_iter()
				.map(|metric| (
					metric.operation,
					metric.count,
					metric.total_us,
					metric.work_units,
					metric.samples_us,
				))
				.collect::<Vec<_>>(),
		),
		(
			Some(10),
			Some(40),
			Some(30),
			Some(30),
			std::collections::BTreeMap::from([
				("catalogue.misses".into(), 1),
				("commands.download.success".into(), 1),
				("downloads.batches".into(), 1),
				("downloads.bytes".into(), 42),
				("downloads.episodes.downloaded".into(), 1),
				("downloads.episodes.skipped".into(), 1),
			]),
			std::collections::BTreeMap::from([(
				"downloads.batch_duration_ms".into(),
				vec![12],
			)]),
			vec![("search.rank".into(), 1, 50, 30_000, vec![50])],
		)
	);
	store.clear(ClearRange::All, 50_000).await.unwrap();
	assert!(
		crate::telemetry::snapshot::capture(&store)
			.await
			.unwrap()
			.counters
			.is_empty()
	);

	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn partial_clear_is_inclusive_cascades_and_advances_its_watermark() {
	let paths = paths("partial-clear");
	let store = Store::open(paths.clone()).await.unwrap();
	sqlx::raw_sql(
		"INSERT INTO command_events(invocation_id, observed_at_ms, command, outcome)
		 VALUES
		 ('00000000-0000-7000-8000-000000000000', 9, 'download', 'success'),
		 ('00000000-0000-7000-8000-000000000000', 10, 'download', 'success'),
		 ('00000000-0000-7000-8000-000000000000', 30, 'download', 'success');
		 INSERT INTO series_selection_events(
		  invocation_id, observed_at_ms, series_source, series_id, series_title,
		  identity_redacted)
		 SELECT invocation_id, observed_at_ms, 'anime365', 365, 'Series', 0
		 FROM command_events;
		 INSERT INTO download_batches(
		  invocation_id, observed_at_ms, series_source, series_id, series_title,
		  identity_redacted, duration_us)
		 SELECT invocation_id, observed_at_ms, 'anime365', 365, 'Series', 0, 1
		 FROM command_events;
		 INSERT INTO download_outcomes(batch_id, status, downloaded_bytes)
		 SELECT id, 'downloaded', 1 FROM download_batches;
		 INSERT INTO performance_events(
		  invocation_id, observed_at_ms, operation, duration_us)
		 SELECT invocation_id, observed_at_ms, 'search.rank', 1 FROM command_events;",
	)
	.execute(&store.pool)
	.await
	.unwrap();
	let before = store.collection_state().await.unwrap();

	store.clear(ClearRange::Since(10), 20).await.unwrap();
	store.clear(ClearRange::Since(100), 21).await.unwrap();
	store.clear(ClearRange::Since(100), 19).await.unwrap();

	assert_eq!(
		(
			stored_event_times(&store).await,
			store.collection_state().await.unwrap()
		),
		(
			(
				vec![
					("command".into(), 9),
					("download".into(), 9),
					("performance".into(), 9),
					("series".into(), 9),
				],
				1,
			),
			CollectionState {
				last_cleared_at_ms: Some(21),
				..before
			}
		)
	);

	store.close().await;
	cleanup(&paths);
}

#[tokio::test]
async fn rolls_back_incomplete_download_batches_and_enforces_scalars() {
	let paths = paths("atomic");
	let store = Store::open(paths.clone()).await.unwrap();
	let invocation_id = InvocationId::new();
	let mut watermark = None;
	let error = store
		.commit(
			&mut watermark,
			vec![
				Observation {
					invocation_id,
					observed_at_ms: 1,
					kind: ObservationKind::Command {
						command: Command::Download,
						outcome: CommandOutcome::Success,
					},
				},
				Observation {
					invocation_id,
					observed_at_ms: 2,
					kind: ObservationKind::DownloadBatch {
						identity: SeriesIdentity::Included {
							source: ContentSource::Anime365,
							id: 365,
							title: "Series".into(),
						},
						duration_us: 3,
						outcomes: vec![DownloadOutcome {
							status: Status::Downloaded,
							bytes: None,
						}],
					},
				},
			],
		)
		.await
		.unwrap_err();

	assert!(error.to_string().contains("Could not update"));
	assert_eq!(
		(
			sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM command_events")
				.fetch_one(&store.pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>(
				"SELECT COUNT(*) FROM download_batches"
			)
			.fetch_one(&store.pool)
			.await
			.unwrap(),
			sqlx::query_scalar::<_, i64>(
				"SELECT COUNT(*) FROM download_outcomes",
			)
			.fetch_one(&store.pool)
			.await
			.unwrap(),
			sqlx::query(
				"INSERT INTO series_selection_events \
				 (invocation_id, observed_at_ms, series_source, series_id, \
				 series_title, identity_redacted) VALUES \
				 ('00000000-0000-7000-8000-000000000000', 0, \
				 'anime365', 0, 'x', 0)",
			)
			.execute(&store.pool)
			.await
			.is_err(),
		),
		(0, 0, 0, true)
	);

	store.close().await;
	cleanup(&paths);
}

async fn stored_event_times(store: &Store) -> (Vec<(String, i64)>, i64) {
	(
		sqlx::query_as(
			"SELECT 'command', observed_at_ms FROM command_events \
			 UNION ALL SELECT 'series', observed_at_ms FROM series_selection_events \
			 UNION ALL SELECT 'download', observed_at_ms FROM download_batches \
			 UNION ALL SELECT 'performance', observed_at_ms FROM performance_events \
			 ORDER BY 1, 2",
		)
		.fetch_all(&store.pool)
		.await
		.unwrap(),
		sqlx::query_scalar("SELECT COUNT(*) FROM download_outcomes")
			.fetch_one(&store.pool)
			.await
			.unwrap(),
	)
}

fn paths(name: &str) -> Paths {
	let root = std::env::temp_dir().join(format!(
		"a365-telemetry-storage-{name}-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	Paths {
		data: root.join("data/telemetry.sqlite"),
		disabled: root.join("config/telemetry-disabled"),
		lock: root.join("data/telemetry.lock"),
	}
}

fn identity() -> SeriesIdentity {
	SeriesIdentity::Included {
		source: ContentSource::Anime365,
		id: 365,
		title: "Private Series title".into(),
	}
}

fn cleanup(paths: &Paths) {
	fs::remove_dir_all(paths.data.parent().unwrap().parent().unwrap()).unwrap();
}
