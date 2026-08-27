use pretty_assertions::assert_eq;

use super::{TrendingData, TrendingSeries, retain_non_adult};

#[test]
fn trending_response_preserves_rank_and_represents_unclassified_adult_state() {
	let response = serde_json::from_value::<TrendingData>(serde_json::json!({
		"Page": { "media": [
			{
				"id": 10,
				"idMal": 20,
				"isAdult": false,
				"trending": 900,
				"title": { "userPreferred": "Safe" }
			},
			{
				"id": 11,
				"idMal": null,
				"isAdult": null,
				"trending": 800,
				"title": { "romaji": "Unclassified" }
			}
		] }
	}))
	.unwrap();

	let mut trends = response.page.media;
	retain_non_adult(&mut trends);

	assert_eq!(
		trends,
		vec![TrendingSeries {
			id: 10,
			id_mal: Some(20),
			is_adult: Some(false),
			trending: 900,
			title: crate::anilist::MediaTitle {
				user_preferred: Some("Safe".into()),
				romaji: None,
				english: None,
				native: None,
			},
		}],
	);
}
