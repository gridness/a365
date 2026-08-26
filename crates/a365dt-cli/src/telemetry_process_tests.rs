use std::{
	collections::{BTreeMap, BTreeSet},
	io::{self, Write},
};

use pretty_assertions::assert_eq;

use super::{
	CatalogueUse, Command, CommandOutcome, InvocationId, Operation, Paths,
	SeriesRecording, Writer,
	recording::{Observation, ObservationKind, now_ms},
	snapshot,
	storage::{ClearRange, Store},
};
use crate::{
	api::Series,
	download::{Outcome, Status},
	error::Error,
};

#[tokio::test]
#[ignore]
async fn worker_concurrent_first_open() {
	wait_for_input("OPEN");
	let store = Store::open(paths()).await.unwrap();
	assert_eq!(
		store.collection_state().await.unwrap(),
		super::storage::CollectionState {
			enabled: false,
			last_enabled_at_ms: None,
			last_disabled_at_ms: Some(123_000),
			last_cleared_at_ms: None,
		}
	);
	barrier("OPENED");
	store.close().await;
}

#[tokio::test]
#[ignore]
async fn worker_writer_drain() {
	let (recorder, writer) =
		Writer::at(paths(), InvocationId::new()).await.unwrap();
	barrier("READY");
	wait_for_input("RECORD");
	let series = series();
	recorder.record_command(Command::Download, CommandOutcome::Success);
	recorder.record_series(
		&series,
		CatalogueUse::Hit,
		SeriesRecording::IncludeIdentity,
	);
	recorder.record_download(
		&series,
		&crate::download::Summary {
			outcomes: vec![
				Outcome {
					episode: "private".into(),
					status: Status::Downloaded,
					bytes: 42,
					detail: Error::new("private"),
				},
				Outcome {
					episode: "private".into(),
					status: Status::Skipped,
					bytes: 0,
					detail: Error::new("private"),
				},
			],
			elapsed: std::time::Duration::from_millis(5),
		},
		SeriesRecording::IncludeIdentity,
	);
	drop(recorder.measure_items(Operation::SearchRank, 10));
	barrier("RECORDED");
	wait_for_input("FINISH");
	writer.finish().await.unwrap();
	barrier("FINISHED");
}

#[tokio::test]
#[ignore]
async fn worker_verify_writer_drain() {
	let store = Store::open(paths()).await.unwrap();
	let commands = sqlx::query_as::<_, (String, i64, String, String)>(
		"SELECT invocation_id, observed_at_ms, command, outcome \
		 FROM command_events ORDER BY invocation_id",
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	let selections =
		sqlx::query_as::<_, (String, i64, i64, String, Option<String>)>(
			"SELECT invocation_id, observed_at_ms, series_id, series_title, \
			 catalogue_result FROM series_selection_events \
			 ORDER BY invocation_id",
		)
		.fetch_all(&store.pool)
		.await
		.unwrap();
	let batches = sqlx::query_as::<_, (i64, String, i64, i64, String, i64)>(
		"SELECT id, invocation_id, observed_at_ms, series_id, series_title, \
		 duration_us FROM download_batches ORDER BY invocation_id",
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	let outcomes = sqlx::query_as::<_, (i64, String, Option<i64>)>(
		"SELECT batch_id, status, downloaded_bytes FROM download_outcomes \
		 ORDER BY batch_id, id",
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	let performance =
		sqlx::query_as::<_, (String, i64, String, i64, Option<i64>)>(
			"SELECT invocation_id, observed_at_ms, operation, duration_us, \
			 work_units FROM performance_events ORDER BY invocation_id",
		)
		.fetch_all(&store.pool)
		.await
		.unwrap();
	let invocations = commands
		.iter()
		.map(|(invocation, ..)| invocation.clone())
		.collect::<BTreeSet<_>>();
	let mut batch_ids = batches
		.iter()
		.map(|(batch_id, ..)| *batch_id)
		.collect::<Vec<_>>();
	batch_ids.sort_unstable();
	assert_eq!(
		(
			commands
				.iter()
				.map(|(_, _, command, outcome)| {
					(command.as_str(), outcome.as_str())
				})
				.collect::<Vec<_>>(),
			selections
				.iter()
				.map(|(_, _, id, title, result)| {
					(*id, title.as_str(), result.as_deref())
				})
				.collect::<Vec<_>>(),
			batches
				.iter()
				.map(|(_, _, _, id, title, duration)| {
					(*id, title.as_str(), *duration)
				})
				.collect::<Vec<_>>(),
			outcomes,
			performance
				.iter()
				.map(|(_, _, operation, _, work)| {
					(operation.as_str(), *work)
				})
				.collect::<Vec<_>>(),
		),
		(
			vec![("download", "success"), ("download", "success")],
			vec![(365, "Series", Some("hit")), (365, "Series", Some("hit")),],
			vec![(365, "Series", 5_000), (365, "Series", 5_000)],
			batch_ids
				.into_iter()
				.flat_map(|batch_id| {
					[
						(batch_id, "downloaded".into(), Some(42)),
						(batch_id, "skipped".into(), None),
					]
				})
				.collect::<Vec<_>>(),
			vec![("search.rank", Some(10)), ("search.rank", Some(10))],
		)
	);
	assert_eq!(
		(
			invocations.len(),
			selections
				.iter()
				.map(|(invocation, ..)| invocation.clone())
				.collect::<BTreeSet<_>>(),
			batches
				.iter()
				.map(|(_, invocation, ..)| invocation.clone())
				.collect::<BTreeSet<_>>(),
			performance
				.iter()
				.map(|(invocation, ..)| invocation.clone())
				.collect::<BTreeSet<_>>(),
		),
		(
			2,
			invocations.clone(),
			invocations.clone(),
			invocations.clone(),
		)
	);
	assert!(
		commands.iter().all(|(_, at, ..)| *at >= 0)
			&& selections.iter().all(|(_, at, ..)| *at >= 0)
			&& batches.iter().all(|(_, _, at, ..)| *at >= 0)
			&& performance
				.iter()
				.all(|(_, at, _, duration, _)| *at >= 0 && *duration >= 0)
	);
	store.close().await;
}

#[tokio::test]
#[ignore]
async fn worker_state_batches() {
	let store = Store::open(paths()).await.unwrap();
	let mut watermark =
		store.collection_state().await.unwrap().last_cleared_at_ms;
	let invocation_id = InvocationId::new();
	barrier("READY");
	wait_for_input("DISABLED_BATCH");
	store
		.commit(
			&mut watermark,
			vec![observation(invocation_id, 1, Command::Download)],
		)
		.await
		.unwrap();
	barrier("DISABLED_BATCHED");
	wait_for_input("ENABLED_BATCH");
	store
		.commit(
			&mut watermark,
			vec![observation(invocation_id, 2, Command::Update)],
		)
		.await
		.unwrap();
	barrier("ENABLED_BATCHED");
	wait_for_input("WATERMARK_BATCH");
	let cleared_at = store
		.collection_state()
		.await
		.unwrap()
		.last_cleared_at_ms
		.unwrap();
	store
		.commit(
			&mut watermark,
			vec![
				observation(invocation_id, cleared_at, Command::Doctor),
				observation(invocation_id, cleared_at + 1, Command::Stats),
			],
		)
		.await
		.unwrap();
	assert_eq!(
		snapshot::capture(&store).await.unwrap().counters,
		BTreeMap::from([("commands.stats.success".into(), 1)])
	);
	barrier("VERIFIED");
	store.close().await;
}

#[tokio::test]
#[ignore]
async fn worker_state_control() {
	let store = Store::open(paths()).await.unwrap();
	barrier("READY");
	loop {
		match read_input().as_str() {
			"DISABLE" => {
				store.disable(InvocationId::new()).await.unwrap();
				barrier("DISABLED");
			}
			"ENABLE" => {
				store.enable(InvocationId::new()).await.unwrap();
				barrier("ENABLED");
			}
			"CLEAR" => {
				store.clear(ClearRange::All, now_ms()).await.unwrap();
				barrier("CLEARED");
			}
			"FINISH" => break,
			input => panic!("unexpected control input: {input}"),
		}
	}
	store.close().await;
}

#[tokio::test]
#[ignore]
async fn worker_migration_open() {
	match Store::open(paths()).await {
		Ok(store) => {
			store.close().await;
			barrier("OPENED");
		}
		Err(_) => barrier("BLOCKED"),
	}
}

fn paths() -> Paths {
	let root = std::env::current_dir().unwrap();
	Paths {
		data: root.join("telemetry.sqlite"),
		lock: root.join("telemetry.lock"),
		disabled: root.join("telemetry-disabled"),
	}
}

fn observation(
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

fn series() -> Series {
	Series {
		source: crate::content::ContentSource::Anime365,
		id: 365,
		title: "Series".into(),
		year: None,
		type_title: None,
		number_of_episodes: None,
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: None,
		episodes: Vec::new(),
	}
}

fn wait_for_input(expected: &str) {
	assert_eq!(read_input(), expected);
}

fn read_input() -> String {
	let mut line = String::new();
	io::stdin().read_line(&mut line).unwrap();
	line.trim().into()
}

fn barrier(token: &str) {
	println!("\n{token}");
	io::stdout().flush().unwrap();
}
