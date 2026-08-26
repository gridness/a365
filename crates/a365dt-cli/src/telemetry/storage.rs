use std::{
	fs::{self, File, OpenOptions},
	io,
	path::Path,
};

use sqlx::{Sqlite, SqlitePool, Transaction, migrate::Migrator};

use super::{
	Observation, ObservationKind, Paths,
	recording::{DownloadOutcome, now_ms},
};
use crate::{
	download::Status,
	error::Error,
	sqlite::{self, Durability, MigrationError, OpenMode},
};

mod clearing;

pub(super) use clearing::ClearRange;

const INITIALIZATION_LOCK: &str = "telemetry-initialization.lock";
pub(super) static MIGRATOR: Migrator = sqlx::migrate!("./migrations/telemetry");
type IdentityFields = (Option<&'static str>, Option<i64>, Option<String>, bool);

#[derive(Clone)]
pub(super) struct Store {
	pub(super) pool: SqlitePool,
	pub(super) paths: Paths,
	warning: Option<Error>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CollectionState {
	pub(super) enabled: bool,
	pub(super) last_enabled_at_ms: Option<u64>,
	pub(super) last_disabled_at_ms: Option<u64>,
	pub(super) last_cleared_at_ms: Option<u64>,
}

impl Store {
	pub(super) async fn open(paths: Paths) -> Result<Self, Error> {
		let Some(directory) = paths.data.parent() else {
			return Err(Error::new(
				"Could not resolve the local telemetry directory.",
			));
		};
		fs::create_dir_all(directory).map_err(open_error)?;
		let _lock = initialization_lock(directory)?;
		let mode = if paths.data.exists() {
			OpenMode::Existing
		} else {
			OpenMode::Initialize
		};
		let pool = sqlite::connect(&paths.data, mode, Durability::Telemetry)
			.await
			.map_err(open_error)?;
		if let Err(error) = migrate(&pool, &paths).await {
			pool.close().await;
			if matches!(mode, OpenMode::Initialize) {
				sqlite::remove_new_database(&paths.data);
			}
			return Err(error);
		}
		if let Err(error) = crate::app_files::private_file(&paths.data) {
			pool.close().await;
			return Err(open_error(error));
		}
		let warning = retire_legacy_files(&paths).err();
		Ok(Self {
			pool,
			paths,
			warning,
		})
	}

	pub(super) async fn close(self) {
		self.pool.close().await;
	}

	pub(super) fn warning(&self) -> Option<Error> {
		self.warning.clone()
	}

	pub(super) async fn collection_state(
		&self,
	) -> Result<CollectionState, Error> {
		let (
			enabled,
			last_enabled_at_ms,
			last_disabled_at_ms,
			last_cleared_at_ms,
		) = sqlx::query_as::<_, (bool, Option<i64>, Option<i64>, Option<i64>)>(
			"SELECT enabled, last_enabled_at_ms, last_disabled_at_ms, \
				 last_cleared_at_ms FROM collection_state WHERE singleton = 1",
		)
		.fetch_one(&self.pool)
		.await
		.map_err(read_error)?;
		Ok(CollectionState {
			enabled,
			last_enabled_at_ms: optional_u64(
				last_enabled_at_ms,
				"enable time",
			)?,
			last_disabled_at_ms: optional_u64(
				last_disabled_at_ms,
				"disable time",
			)?,
			last_cleared_at_ms: optional_u64(
				last_cleared_at_ms,
				"clear watermark",
			)?,
		})
	}

	pub(super) async fn commit(
		&self,
		watermark: &mut Option<u64>,
		observations: Vec<Observation>,
	) -> Result<(), Error> {
		let mut transaction = self
			.pool
			.begin_with("BEGIN IMMEDIATE")
			.await
			.map_err(write_error)?;
		let (enabled, cleared_at_ms) =
			sqlx::query_as::<_, (bool, Option<i64>)>(
				"SELECT enabled, last_cleared_at_ms FROM collection_state \
			 WHERE singleton = 1",
			)
			.fetch_one(&mut *transaction)
			.await
			.map_err(read_error)?;
		if !enabled {
			return transaction.commit().await.map_err(write_error);
		}
		let cleared_at_ms = cleared_at_ms
			.map(|value| u64_from(value, "clear watermark"))
			.transpose()?;
		if cleared_at_ms > *watermark {
			*watermark = cleared_at_ms;
		}
		for observation in observations.into_iter().filter(|observation| {
			watermark.is_none_or(|at| observation.observed_at_ms > at)
		}) {
			insert(&mut transaction, observation).await?;
		}
		transaction.commit().await.map_err(write_error)
	}

	pub(super) async fn disable(
		&self,
		invocation_id: super::InvocationId,
	) -> Result<(), Error> {
		let at = now_ms();
		let mut transaction = self
			.pool
			.begin_with("BEGIN IMMEDIATE")
			.await
			.map_err(write_error)?;
		let enabled = sqlx::query_scalar::<_, bool>(
			"SELECT enabled FROM collection_state WHERE singleton = 1",
		)
		.fetch_one(&mut *transaction)
		.await
		.map_err(read_error)?;
		if enabled {
			insert(
				&mut transaction,
				Observation {
					invocation_id,
					observed_at_ms: at,
					kind: ObservationKind::Command {
						command: super::Command::TelemetryDisable,
						outcome: super::CommandOutcome::Success,
					},
				},
			)
			.await?;
		}
		sqlx::query(
			"UPDATE collection_state SET enabled = 0, \
			 last_disabled_at_ms = ? WHERE singleton = 1",
		)
		.bind(i64_from(at, "disable timestamp")?)
		.execute(&mut *transaction)
		.await
		.map_err(write_error)?;
		transaction.commit().await.map_err(write_error)
	}

	pub(super) async fn enable(
		&self,
		invocation_id: super::InvocationId,
	) -> Result<(), Error> {
		let at = now_ms();
		let mut transaction = self
			.pool
			.begin_with("BEGIN IMMEDIATE")
			.await
			.map_err(write_error)?;
		sqlx::query(
			"UPDATE collection_state SET enabled = 1, \
			 last_enabled_at_ms = ? WHERE singleton = 1",
		)
		.bind(i64_from(at, "enable timestamp")?)
		.execute(&mut *transaction)
		.await
		.map_err(write_error)?;
		insert(
			&mut transaction,
			Observation {
				invocation_id,
				observed_at_ms: at,
				kind: ObservationKind::Command {
					command: super::Command::TelemetryEnable,
					outcome: super::CommandOutcome::Success,
				},
			},
		)
		.await?;
		transaction.commit().await.map_err(write_error)
	}
}

async fn insert(
	transaction: &mut Transaction<'_, Sqlite>,
	observation: Observation,
) -> Result<(), Error> {
	let invocation_id = observation.invocation_id.to_string();
	let observed_at_ms = i64_from(observation.observed_at_ms, "timestamp")?;
	match observation.kind {
		ObservationKind::Command { command, outcome } => {
			sqlx::query(
				"INSERT INTO command_events \
				 (invocation_id, observed_at_ms, command, outcome) \
				 VALUES (?, ?, ?, ?)",
			)
			.bind(invocation_id)
			.bind(observed_at_ms)
			.bind(command.database_name())
			.bind(outcome.name())
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?;
		}
		ObservationKind::SeriesSelection {
			identity,
			catalogue,
		} => {
			let (source, series_id, series_title, identity_redacted) =
				identity_fields(identity)?;
			sqlx::query(
				"INSERT INTO series_selection_events \
				 (invocation_id, observed_at_ms, series_source, series_id, \
				 series_title, identity_redacted, catalogue_result) \
				 VALUES (?, ?, ?, ?, ?, ?, ?)",
			)
			.bind(invocation_id)
			.bind(observed_at_ms)
			.bind(source)
			.bind(series_id)
			.bind(series_title)
			.bind(identity_redacted)
			.bind(catalogue.and_then(|usage| usage.database_name()))
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?;
		}
		ObservationKind::DownloadBatch {
			identity,
			duration_us,
			outcomes,
		} => {
			let (source, series_id, series_title, identity_redacted) =
				identity_fields(identity)?;
			let batch_id = sqlx::query(
				"INSERT INTO download_batches \
				 (invocation_id, observed_at_ms, series_source, series_id, \
				 series_title, identity_redacted, duration_us) \
				 VALUES (?, ?, ?, ?, ?, ?, ?)",
			)
			.bind(invocation_id)
			.bind(observed_at_ms)
			.bind(source)
			.bind(series_id)
			.bind(series_title)
			.bind(identity_redacted)
			.bind(i64_from(duration_us, "Download duration")?)
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?
			.last_insert_rowid();
			for outcome in outcomes {
				insert_download_outcome(transaction, batch_id, outcome).await?;
			}
		}
		ObservationKind::Playback {
			identity,
			duration_us,
			outcome,
		} => {
			let (source, series_id, series_title, identity_redacted) =
				identity_fields(identity)?;
			sqlx::query(
				"INSERT INTO playback_sessions \
				 (invocation_id, observed_at_ms, series_source, series_id, \
				 series_title, identity_redacted, duration_us, outcome) \
				 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
			)
			.bind(invocation_id)
			.bind(observed_at_ms)
			.bind(source)
			.bind(series_id)
			.bind(series_title)
			.bind(identity_redacted)
			.bind(i64_from(duration_us, "Playback duration")?)
			.bind(outcome.name())
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?;
		}
		ObservationKind::Performance {
			operation,
			duration_us,
			work_units,
		} => {
			sqlx::query(
				"INSERT INTO performance_events \
				 (invocation_id, observed_at_ms, operation, duration_us, \
				 work_units) VALUES (?, ?, ?, ?, ?)",
			)
			.bind(invocation_id)
			.bind(observed_at_ms)
			.bind(operation.name())
			.bind(i64_from(duration_us, "operation duration")?)
			.bind(
				work_units
					.map(|value| i64_from(value, "operation work units"))
					.transpose()?,
			)
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?;
		}
	}
	Ok(())
}

fn identity_fields(
	identity: super::recording::SeriesIdentity,
) -> Result<IdentityFields, Error> {
	match identity {
		super::recording::SeriesIdentity::AggregateOnly => {
			Ok((None, None, None, true))
		}
		super::recording::SeriesIdentity::Included { source, id, title } => {
			Ok((
				Some(source.as_str()),
				Some(i64_from(id, "Series ID")?),
				Some(title),
				false,
			))
		}
	}
}

async fn insert_download_outcome(
	transaction: &mut Transaction<'_, Sqlite>,
	batch_id: i64,
	outcome: DownloadOutcome,
) -> Result<(), Error> {
	let (status, downloaded_bytes) = match outcome.status {
		Status::Downloaded => ("downloaded", outcome.bytes),
		Status::Skipped => ("skipped", None),
		Status::Failed => ("failed", None),
		Status::MuxFailed => ("mux_failed", None),
		Status::Interrupted => ("interrupted", None),
	};
	sqlx::query(
		"INSERT INTO download_outcomes \
		 (batch_id, status, downloaded_bytes) VALUES (?, ?, ?)",
	)
	.bind(batch_id)
	.bind(status)
	.bind(
		downloaded_bytes
			.map(|value| i64_from(value, "downloaded bytes"))
			.transpose()?,
	)
	.execute(&mut **transaction)
	.await
	.map_err(write_error)?;
	Ok(())
}

async fn migrate(pool: &SqlitePool, paths: &Paths) -> Result<(), Error> {
	let (mut transaction, initialized) =
		sqlite::begin_migrations(pool, &MIGRATOR, "telemetry")
			.await
			.map_err(migration_error)?;
	if initialized {
		let disabled_at_ms = legacy_disabled_at(paths)?;
		let enabled_at_ms = disabled_at_ms
			.is_none()
			.then(now_ms)
			.map(|timestamp| i64_from(timestamp, "enable timestamp"))
			.transpose()?;
		sqlx::query(
			"UPDATE collection_state SET enabled = ?, \
			 last_enabled_at_ms = ?, last_disabled_at_ms = ? \
			 WHERE singleton = 1",
		)
		.bind(disabled_at_ms.is_none())
		.bind(enabled_at_ms)
		.bind(disabled_at_ms)
		.execute(&mut *transaction)
		.await
		.map_err(write_error)?;
	}
	transaction.commit().await.map_err(write_error)?;
	validate_schema(pool).await
}

fn legacy_disabled_at(paths: &Paths) -> Result<Option<i64>, Error> {
	if !paths.disabled.try_exists().map_err(open_error)? {
		return Ok(None);
	}
	let timestamp = fs::read_to_string(&paths.disabled)
		.map_err(open_error)?
		.parse::<u64>()
		.map_err(|error| {
			open_error(format!(
				"the local telemetry opt-out timestamp is invalid: {error}"
			))
		})?
		.checked_mul(1_000)
		.ok_or_else(|| {
			open_error("the local telemetry opt-out timestamp is out of range")
		})?;
	i64::try_from(timestamp).map(Some).map_err(|error| {
		open_error(format!(
			"the local telemetry opt-out timestamp is out of range: {error}"
		))
	})
}

async fn validate_schema(pool: &SqlitePool) -> Result<(), Error> {
	let state = sqlx::query_scalar::<_, i64>(
		"SELECT COUNT(*) FROM collection_state WHERE singleton = 1",
	)
	.fetch_one(pool)
	.await
	.map_err(read_error)?;
	if state != 1 {
		return Err(read_error("telemetry collection state is missing"));
	}
	Ok(())
}

fn initialization_lock(directory: &Path) -> Result<File, Error> {
	let file = OpenOptions::new()
		.create(true)
		.truncate(false)
		.read(true)
		.write(true)
		.open(directory.join(INITIALIZATION_LOCK))
		.map_err(open_error)?;
	file.lock().map_err(open_error)?;
	Ok(file)
}

fn retire_legacy_files(paths: &Paths) -> Result<(), Error> {
	for path in [
		paths.data.with_file_name("telemetry.json"),
		paths.lock.clone(),
		paths.disabled.clone(),
	] {
		match fs::remove_file(&path) {
			Ok(()) => {}
			Err(error) if error.kind() == io::ErrorKind::NotFound => {}
			Err(error) => {
				return Err(Error::with_debug(
					"Could not retire an obsolete local telemetry file; it will be ignored.",
					format!("{}: {error}", path.display()),
				));
			}
		}
	}
	Ok(())
}

fn migration_error(error: MigrationError) -> Error {
	match error {
		MigrationError::Database(error) => open_error(error),
		MigrationError::Invalid(detail) => open_error(detail),
	}
}

fn open_error(error: impl std::fmt::Display) -> Error {
	Error::with_debug(
		"Could not open the local telemetry. Run `a365 doctor --debug` to inspect its database.",
		error,
	)
}

pub(super) fn read_error(error: impl std::fmt::Display) -> Error {
	Error::with_debug(
		"Could not read the local telemetry. Run `a365 doctor --debug` to inspect its database.",
		error,
	)
}

fn write_error(error: impl std::fmt::Display) -> Error {
	Error::with_debug(
		"Could not update the local telemetry. Close other a365 processes and retry.",
		error,
	)
}

fn i64_from(value: u64, name: &str) -> Result<i64, Error> {
	i64::try_from(value).map_err(|error| {
		write_error(format!("{name} is out of range: {error}"))
	})
}

fn u64_from(value: i64, name: &str) -> Result<u64, Error> {
	u64::try_from(value)
		.map_err(|error| read_error(format!("{name} is out of range: {error}")))
}

fn optional_u64(value: Option<i64>, name: &str) -> Result<Option<u64>, Error> {
	value.map(|value| u64_from(value, name)).transpose()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
