use std::path::{Path, PathBuf};

use reqwest::Response;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::error::Error;

pub(super) struct LocalSubtitle {
	path: PathBuf,
}

impl LocalSubtitle {
	pub(super) async fn fetch(url: &str) -> Result<Self, Error> {
		let response = reqwest::get(url)
			.await
			.and_then(Response::error_for_status)
			.map_err(|error| {
				Error::with_debug(
					"Could not load the selected subtitles for IINA.",
					error.without_url(),
				)
			})?;
		Self::write(response).await
	}

	pub(super) fn path(&self) -> &Path {
		&self.path
	}

	async fn write(mut response: Response) -> Result<Self, Error> {
		let path = std::env::temp_dir()
			.join(format!("a365-{}.ass", Uuid::now_v7().simple()));
		let mut file = fs::OpenOptions::new()
			.create_new(true)
			.write(true)
			.open(&path)
			.await
			.map_err(local_subtitle_error)?;
		let subtitle = Self { path };
		while let Some(chunk) = response.chunk().await.map_err(|error| {
			Error::with_debug(
				"Could not load the selected subtitles for IINA.",
				error.without_url(),
			)
		})? {
			file.write_all(&chunk).await.map_err(local_subtitle_error)?;
		}
		file.flush().await.map_err(local_subtitle_error)?;
		Ok(subtitle)
	}
}

impl Drop for LocalSubtitle {
	fn drop(&mut self) {
		let _ = std::fs::remove_file(&self.path);
	}
}

fn local_subtitle_error(error: std::io::Error) -> Error {
	Error::with_debug(
		"Could not prepare the selected subtitles for IINA.",
		error,
	)
}

#[cfg(test)]
#[path = "subtitle_tests.rs"]
mod tests;
