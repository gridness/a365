#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::{
	api::{Episode, Series},
	app_files,
	content::SeriesKey,
	error::Error,
	playback::Position,
	select::{PlannedRelease, TrackKey},
};

const FILE_NAME: &str = "continue-watching.json";
const VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Entry {
	pub series: SeriesKey,
	pub series_title: String,
	pub episode_id: u64,
	pub episode_label: String,
	pub track: TrackKey,
	pub height: u16,
	#[serde(default, skip_serializing_if = "Position::at_start")]
	pub position: Position,
}

#[derive(Deserialize, Serialize)]
struct Document {
	version: u8,
	entry: Entry,
}

#[derive(Clone)]
pub(crate) struct Store {
	path: PathBuf,
}

impl Entry {
	pub(crate) fn new(
		series: &Series,
		release: &PlannedRelease,
		track: &TrackKey,
	) -> Self {
		Self {
			series: series.key(),
			series_title: series.title.clone(),
			episode_id: release.episode.id,
			episode_label: release.episode.episode_full.clone(),
			track: track.clone(),
			height: release.height,
			position: Position::START,
		}
	}

	pub(crate) fn with_position(&self, position: Position) -> Self {
		Self {
			position,
			..self.clone()
		}
	}

	pub(crate) fn with_episode(&self, episode: &Episode) -> Self {
		Self {
			episode_id: episode.id,
			episode_label: episode.episode_full.clone(),
			position: Position::START,
			..self.clone()
		}
	}
}

impl Store {
	pub(crate) fn discover() -> Result<Self, Error> {
		let directory = app_files::data_directory().ok_or_else(|| {
			Error::new("Could not resolve the local Continue Watching store.")
		})?;
		Ok(Self {
			path: directory.join(FILE_NAME),
		})
	}

	pub(crate) async fn load(&self) -> Result<Option<Entry>, Error> {
		let bytes = match fs::read(&self.path).await {
			Ok(bytes) => bytes,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				return Ok(None);
			}
			Err(error) => return Err(state_error(error)),
		};
		let document =
			serde_json::from_slice::<Document>(&bytes).map_err(|error| {
				Error::with_debug(
					"Continue Watching state is unreadable.",
					error,
				)
			})?;
		if document.version != VERSION {
			return Err(Error::new(
				"Continue Watching state was written by an unsupported a365 version.",
			));
		}
		Ok(Some(document.entry))
	}

	pub(crate) async fn save(&self, entry: &Entry) -> Result<(), Error> {
		let bytes = serde_json::to_vec_pretty(&Document {
			version: VERSION,
			entry: entry.clone(),
		})
		.map_err(|error| {
			Error::with_debug(
				"Could not prepare Continue Watching state.",
				error,
			)
		})?;
		let temporary = self
			.path
			.with_extension(format!("{}.tmp", Uuid::now_v7().simple()));
		fs::write(&temporary, bytes).await.map_err(state_error)?;
		if let Err(error) = app_files::private_file(&temporary) {
			let _ = fs::remove_file(&temporary).await;
			return Err(state_error(error));
		}
		if let Err(error) = fs::rename(&temporary, &self.path).await {
			let _ = fs::remove_file(&temporary).await;
			return Err(state_error(error));
		}
		Ok(())
	}

	pub(crate) async fn clear(&self) -> Result<(), Error> {
		match fs::remove_file(&self.path).await {
			Ok(()) => Ok(()),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				Ok(())
			}
			Err(error) => Err(state_error(error)),
		}
	}

	#[cfg(test)]
	fn at(path: impl AsRef<Path>) -> Self {
		Self {
			path: path.as_ref().join(FILE_NAME),
		}
	}
}

fn state_error(error: std::io::Error) -> Error {
	Error::with_debug("Could not update Continue Watching state.", error)
}

#[cfg(test)]
#[path = "continue_watching_tests.rs"]
mod tests;
