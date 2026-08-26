use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use sqlx::SqliteConnection;
use tokio::sync::mpsc;

use super::{
	InvocationId, Operation, Recorder,
	storage::{Store, read_error},
};
use crate::{error::Error, sqlite};

pub struct Snapshot {
	pub enabled: bool,
	pub data_path: PathBuf,
	pub disabled_path: PathBuf,
	pub data_bytes: Option<u64>,
	pub schema_version: u16,
	pub first_recorded_at: Option<u64>,
	pub last_recorded_at: Option<u64>,
	pub first_download_at: Option<u64>,
	pub last_download_at: Option<u64>,
	pub last_enabled_at: Option<u64>,
	pub last_disabled_at: Option<u64>,
	pub last_cleared_at: Option<u64>,
	pub counters: BTreeMap<String, u64>,
	pub samples: BTreeMap<String, Vec<u64>>,
	pub performance: Vec<PerformanceMetric>,
}

pub struct PerformanceMetric {
	pub operation: String,
	pub count: u64,
	pub total_us: u64,
	pub work_units: u64,
	pub samples_us: Vec<u64>,
}

pub struct Overhead {
	pub enabled_ns: u64,
	pub disabled_ns: u64,
	pub added_ns: u64,
}

pub(super) async fn capture(store: &Store) -> Result<Snapshot, Error> {
	let mut transaction = store.pool.begin().await.map_err(read_error)?;
	let (enabled, last_enabled_at, last_disabled_at, last_cleared_at) =
		sqlx::query_as::<_, (bool, Option<i64>, Option<i64>, Option<i64>)>(
			"SELECT enabled, last_enabled_at_ms, last_disabled_at_ms, \
			 last_cleared_at_ms FROM collection_state WHERE singleton = 1",
		)
		.fetch_one(&mut *transaction)
		.await
		.map_err(read_error)?;
	let (first_recorded_at, last_recorded_at) =
		sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
			"SELECT MIN(observed_at_ms), MAX(observed_at_ms) FROM (\
			 SELECT observed_at_ms FROM command_events UNION ALL \
			 SELECT observed_at_ms FROM series_selection_events UNION ALL \
			 SELECT observed_at_ms FROM download_batches UNION ALL \
			 SELECT observed_at_ms FROM playback_sessions UNION ALL \
			 SELECT observed_at_ms FROM performance_events\
			 )",
		)
		.fetch_one(&mut *transaction)
		.await
		.map_err(read_error)?;
	let (first_download_at, last_download_at) =
		sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
			"SELECT MIN(observed_at_ms), MAX(observed_at_ms) \
			 FROM download_batches",
		)
		.fetch_one(&mut *transaction)
		.await
		.map_err(read_error)?;
	let schema_version = sqlx::query_scalar::<_, i64>(
		"SELECT MAX(version) FROM _sqlx_migrations",
	)
	.fetch_one(&mut *transaction)
	.await
	.map_err(read_error)?;
	let counters = counters(&mut transaction).await?;
	let samples = download_samples(&mut transaction).await?;
	let performance = performance(&mut transaction).await?;
	transaction.commit().await.map_err(read_error)?;
	Ok(Snapshot {
		enabled,
		data_path: store.paths.data.clone(),
		disabled_path: store.paths.disabled.clone(),
		data_bytes: Some(sqlite::size(&store.paths.data).map_err(read_error)?),
		schema_version: u16::try_from(schema_version).map_err(read_error)?,
		first_recorded_at: timestamp(first_recorded_at)?,
		last_recorded_at: timestamp(last_recorded_at)?,
		first_download_at: timestamp(first_download_at)?,
		last_download_at: timestamp(last_download_at)?,
		last_enabled_at: timestamp(last_enabled_at)?,
		last_disabled_at: timestamp(last_disabled_at)?,
		last_cleared_at: timestamp(last_cleared_at)?,
		counters,
		samples,
		performance,
	})
}

async fn counters(
	connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, u64>, Error> {
	let mut counters = BTreeMap::new();
	for (command, outcome, count) in sqlx::query_as::<_, (String, String, i64)>(
		"SELECT command, outcome, COUNT(*) FROM command_events \
			 GROUP BY command, outcome",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	{
		counters.insert(
			format!("commands.{}.{}", command.replace('_', "."), outcome),
			u64_from(count)?,
		);
	}
	for (result, count) in sqlx::query_as::<_, (String, i64)>(
		"SELECT catalogue_result, COUNT(*) FROM series_selection_events \
		 WHERE catalogue_result IS NOT NULL GROUP BY catalogue_result",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	{
		let key = match result.as_str() {
			"hit" => "catalogue.hits",
			"miss" => "catalogue.misses",
			value => {
				return Err(read_error(format!(
					"unknown catalogue result: {value}"
				)));
			}
		};
		counters.insert(key.into(), u64_from(count)?);
	}
	let batches =
		sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM download_batches")
			.fetch_one(&mut *connection)
			.await
			.map_err(read_error)?;
	if batches > 0 {
		counters.insert("downloads.batches".into(), u64_from(batches)?);
	}
	for (status, count) in sqlx::query_as::<_, (String, i64)>(
		"SELECT status, COUNT(*) FROM download_outcomes GROUP BY status",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	{
		counters
			.insert(format!("downloads.episodes.{status}"), u64_from(count)?);
	}
	let (downloaded, bytes) = sqlx::query_as::<_, (i64, i64)>(
		"SELECT COUNT(*), COALESCE(SUM(downloaded_bytes), 0) \
		 FROM download_outcomes WHERE downloaded_bytes IS NOT NULL",
	)
	.fetch_one(&mut *connection)
	.await
	.map_err(read_error)?;
	if downloaded > 0 {
		counters.insert("downloads.bytes".into(), u64_from(bytes)?);
	}
	for (outcome, count) in sqlx::query_as::<_, (String, i64)>(
		"SELECT outcome, COUNT(*) FROM playback_sessions GROUP BY outcome",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	{
		counters.insert(
			format!("playback.outcomes.{}", outcome.replace('_', ".")),
			u64_from(count)?,
		);
	}
	Ok(counters)
}

async fn download_samples(
	connection: &mut SqliteConnection,
) -> Result<BTreeMap<String, Vec<u64>>, Error> {
	let samples = sqlx::query_scalar::<_, i64>(
		"SELECT duration_us FROM download_batches \
		 ORDER BY observed_at_ms DESC, id DESC LIMIT 101",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	.into_iter()
	.map(|duration| u64_from(duration).map(|duration| duration / 1_000))
	.collect::<Result<Vec<_>, _>>()?;
	let mut result = BTreeMap::new();
	if !samples.is_empty() {
		result.insert("downloads.batch_duration_ms".into(), samples);
	}
	let playback = sqlx::query_scalar::<_, i64>(
		"SELECT duration_us FROM playback_sessions \
		 ORDER BY observed_at_ms DESC, id DESC LIMIT 101",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	.into_iter()
	.map(|duration| u64_from(duration).map(|duration| duration / 1_000))
	.collect::<Result<Vec<_>, _>>()?;
	if !playback.is_empty() {
		result.insert("playback.duration_ms".into(), playback);
	}
	Ok(result)
}

async fn performance(
	connection: &mut SqliteConnection,
) -> Result<Vec<PerformanceMetric>, Error> {
	let mut samples = BTreeMap::<String, Vec<u64>>::new();
	for (operation, duration) in sqlx::query_as::<_, (String, i64)>(
		"SELECT operation, duration_us FROM (\
		 SELECT operation, duration_us, ROW_NUMBER() OVER (\
		 PARTITION BY operation ORDER BY observed_at_ms DESC, id DESC\
		 ) AS position FROM performance_events\
		 ) WHERE position <= 101 ORDER BY operation, position",
	)
	.fetch_all(&mut *connection)
	.await
	.map_err(read_error)?
	{
		samples
			.entry(operation)
			.or_default()
			.push(u64_from(duration)?);
	}
	let mut performance = Vec::new();
	for (operation, count, total_us, work_units) in
		sqlx::query_as::<_, (String, i64, i64, i64)>(
			"SELECT operation, COUNT(*), SUM(duration_us), \
			 COALESCE(SUM(work_units), 0) FROM performance_events \
			 GROUP BY operation ORDER BY operation",
		)
		.fetch_all(&mut *connection)
		.await
		.map_err(read_error)?
	{
		let mut recent = samples.remove(&operation).unwrap_or_default();
		recent.sort_unstable();
		performance.push(PerformanceMetric {
			operation,
			count: u64_from(count)?,
			total_us: u64_from(total_us)?,
			work_units: u64_from(work_units)?,
			samples_us: recent,
		});
	}
	Ok(performance)
}

pub(super) fn benchmark_overhead(invocation_id: InvocationId) -> Overhead {
	let (observations, _receiver) = mpsc::unbounded_channel();
	let enabled = Recorder::connected(invocation_id, observations);
	let disabled = Recorder::default();
	let enabled_ns = median_overhead(&enabled);
	let disabled_ns = median_overhead(&disabled);
	Overhead {
		enabled_ns,
		disabled_ns,
		added_ns: enabled_ns.saturating_sub(disabled_ns),
	}
}

fn median_overhead(recorder: &Recorder) -> u64 {
	let mut samples = Vec::with_capacity(1_001);
	for _ in 0..1_001 {
		let started = Instant::now();
		drop(recorder.measure(Operation::SearchRank));
		std::hint::black_box(());
		samples.push(
			u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
		);
	}
	samples.sort_unstable();
	samples[samples.len() / 2]
}

fn timestamp(value: Option<i64>) -> Result<Option<u64>, Error> {
	value
		.map(|value| u64_from(value).map(|value| value / 1_000))
		.transpose()
}

fn u64_from(value: i64) -> Result<u64, Error> {
	u64::try_from(value).map_err(read_error)
}
