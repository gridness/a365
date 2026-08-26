use sqlx::{Sqlite, Transaction};

use super::{Store, i64_from, write_error};
use crate::error::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::telemetry) enum ClearRange {
	All,
	Since(u64),
}

impl Store {
	pub(in crate::telemetry) async fn clear(
		&self,
		range: ClearRange,
		cleared_at_ms: u64,
	) -> Result<(), Error> {
		let cleared_at_ms = i64_from(cleared_at_ms, "clear timestamp")?;
		let cutoff_ms = match range {
			ClearRange::All => None,
			ClearRange::Since(cutoff_ms) => {
				Some(i64_from(cutoff_ms, "clear cutoff")?)
			}
		};
		let mut transaction = self
			.pool
			.begin_with("BEGIN IMMEDIATE")
			.await
			.map_err(write_error)?;
		delete(&mut transaction, cutoff_ms).await?;
		sqlx::query(
			"UPDATE collection_state SET last_cleared_at_ms = \
			 CASE WHEN last_cleared_at_ms >= ? THEN last_cleared_at_ms ELSE ? END \
			 WHERE singleton = 1",
		)
		.bind(cleared_at_ms)
		.bind(cleared_at_ms)
		.execute(&mut *transaction)
		.await
		.map_err(write_error)?;
		transaction.commit().await.map_err(write_error)
	}
}

async fn delete(
	transaction: &mut Transaction<'_, Sqlite>,
	cutoff_ms: Option<i64>,
) -> Result<(), Error> {
	for statement in [
		"DELETE FROM command_events WHERE observed_at_ms >= COALESCE(?, 0)",
		"DELETE FROM series_selection_events \
		 WHERE observed_at_ms >= COALESCE(?, 0)",
		"DELETE FROM download_batches WHERE observed_at_ms >= COALESCE(?, 0)",
		"DELETE FROM playback_sessions WHERE observed_at_ms >= COALESCE(?, 0)",
		"DELETE FROM performance_events WHERE observed_at_ms >= COALESCE(?, 0)",
	] {
		sqlx::query(statement)
			.bind(cutoff_ms)
			.execute(&mut **transaction)
			.await
			.map_err(write_error)?;
	}
	Ok(())
}
