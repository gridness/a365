use serde::Deserialize;

use super::{Client, MediaTitle};
use crate::error::Error;

const QUERY: &str = r#"query TrendingAnime {
  Page(page: 1, perPage: 25) {
    media(type: ANIME, isAdult: false, sort: [TRENDING_DESC]) {
      id idMal isAdult trending
      title { userPreferred romaji english native }
    }
  }
}"#;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrendingSeries {
	pub id: u64,
	pub id_mal: Option<u64>,
	pub is_adult: Option<bool>,
	pub trending: u64,
	pub title: MediaTitle,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TrendingData {
	page: TrendingPage,
}

#[derive(Deserialize)]
struct TrendingPage {
	media: Vec<TrendingSeries>,
}

pub(crate) async fn trending_series() -> Result<Vec<TrendingSeries>, Error> {
	let mut trends = Client::public()?
		.query::<TrendingData, _>(QUERY, serde_json::json!({}))
		.await?
		.page
		.media;
	retain_non_adult(&mut trends);
	Ok(trends)
}

fn retain_non_adult(trends: &mut Vec<TrendingSeries>) {
	trends.retain(|series| series.is_adult == Some(false));
}

#[cfg(test)]
#[path = "trending_tests.rs"]
mod tests;
