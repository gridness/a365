use std::path::PathBuf;

use chrono::Local;

use crate::{app_files, error::Error, ui};

mod clearing;
mod display;
mod migration;
mod recording;
mod snapshot;
mod storage;
mod writer;

pub(crate) use clearing::{ClearRequest, FullClearPermission};
use clearing::{
	PreparedClear, TerminalAccess, authorize_full_clear, prepare_all,
	prepare_since,
};
pub(crate) use display::format_timestamp;
pub(crate) use migration::{
	MigrationPreparation, TelemetryRecovery, ensure_migration_idle,
	prepare_migration_at, recreate_migration_at,
};
pub(crate) use recording::{
	CatalogueUse, Command, CommandOutcome, InvocationId, Operation,
	PlaybackOutcome, Recorder, SeriesRecording,
};
use recording::{Observation, ObservationKind};
pub(crate) use snapshot::{PerformanceMetric, Snapshot};
use storage::Store;
pub(crate) use writer::Writer;

#[derive(Clone, Debug)]
pub(super) struct Paths {
	data: PathBuf,
	disabled: PathBuf,
	lock: PathBuf,
}

impl Paths {
	fn at(data_directory: PathBuf) -> Self {
		Self {
			data: data_directory.join("telemetry.sqlite"),
			lock: data_directory.join("telemetry.lock"),
			disabled: data_directory.join("telemetry-disabled"),
		}
	}

	fn discover() -> Result<Self, Error> {
		let data_directory = app_files::data_directory().ok_or_else(|| {
			Error::new("Could not resolve the local telemetry directory.")
		})?;
		Ok(Self::at(data_directory))
	}
}

pub async fn show(invocation_id: InvocationId) -> Result<(), Error> {
	let store = Store::open(Paths::discover()?).await?;
	warn_cleanup(&store);
	let result = async {
		let snapshot = snapshot::capture(&store).await?;
		display::print(&snapshot);
		if snapshot.enabled {
			let mut watermark =
				store.collection_state().await?.last_cleared_at_ms;
			store
				.commit(
					&mut watermark,
					vec![Observation::command(
						invocation_id,
						Command::TelemetryShow,
						CommandOutcome::Success,
					)],
				)
				.await?;
		}
		Ok(())
	}
	.await;
	store.close().await;
	result
}

pub async fn clear(request: ClearRequest) -> Result<(), Error> {
	let prepared = match request {
		ClearRequest::All(permission) => {
			if !authorize_full_clear(
				permission,
				TerminalAccess::detect(),
				|| ui::confirm("Clear all local telemetry history?", false),
			)? {
				ui::note("Telemetry clear cancelled.");
				return Ok(());
			}
			prepare_all(Local::now())?
		}
		ClearRequest::Since(values) => prepare_since(&values, Local::now())?,
	};
	let (range, cleared_at_ms) = match &prepared {
		PreparedClear::All { cleared_at_ms } => {
			(storage::ClearRange::All, *cleared_at_ms)
		}
		PreparedClear::Since {
			cleared_at_ms,
			cutoff_ms,
			..
		} => (storage::ClearRange::Since(*cutoff_ms), *cleared_at_ms),
	};
	let store = Store::open(Paths::discover()?).await?;
	warn_cleanup(&store);
	let result = store.clear(range, cleared_at_ms).await;
	store.close().await;
	result?;
	match prepared {
		PreparedClear::All { .. } => ui::success("Local telemetry cleared"),
		PreparedClear::Since { expression, .. } => {
			ui::success(format!("Local telemetry since {expression} cleared"));
		}
	}
	Ok(())
}

pub async fn disable(invocation_id: InvocationId) -> Result<(), Error> {
	let store = Store::open(Paths::discover()?).await?;
	warn_cleanup(&store);
	let result = store.disable(invocation_id).await;
	store.close().await;
	result?;
	ui::success("Local telemetry disabled");
	Ok(())
}

pub async fn enable(invocation_id: InvocationId) -> Result<(), Error> {
	let store = Store::open(Paths::discover()?).await?;
	warn_cleanup(&store);
	let result = store.enable(invocation_id).await;
	store.close().await;
	result?;
	ui::success("Local telemetry enabled");
	Ok(())
}

fn warn_cleanup(store: &Store) {
	if let Some(error) = store.warning() {
		ui::warning(error);
	}
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "telemetry/clearing_tests.rs"]
mod clearing_tests;

#[cfg(test)]
#[path = "telemetry_process_tests.rs"]
mod process_tests;
