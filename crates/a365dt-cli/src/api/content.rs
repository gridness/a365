use serde::Deserialize;

use super::Anime365;
use crate::{content::ContentSource, error::Error, telemetry::Operation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdultClassification {
	Adult,
	NonAdult,
	Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesContent {
	is_hentai: Option<u8>,
}

impl Anime365 {
	pub(crate) async fn adult_classification(
		&self,
		series_id: u64,
	) -> Result<AdultClassification, Error> {
		if self.source() == ContentSource::H365 {
			return Ok(AdultClassification::Adult);
		}
		let content = self
			.get_optional::<SeriesContent>(
				&format!("/series/{series_id}"),
				&[],
				false,
				Operation::ApiSeries,
			)
			.await?;
		Ok(content.map_or(AdultClassification::Unknown, |content| {
			content.classification()
		}))
	}
}

impl SeriesContent {
	fn classification(&self) -> AdultClassification {
		match self.is_hentai {
			Some(1) => AdultClassification::Adult,
			Some(0) => AdultClassification::NonAdult,
			Some(_) | None => AdultClassification::Unknown,
		}
	}
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
