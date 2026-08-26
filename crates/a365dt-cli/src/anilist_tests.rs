use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pretty_assertions::assert_eq;

use super::{
	Library, ListEntry, ListGroup, ListStatus, Media, MediaTitle, NextAiring,
	filter_adult, token_expiry,
};
use crate::preferences::AdultContent;

#[test]
fn parses_standard_and_custom_lists_with_private_entry_details() {
	let library = serde_json::from_str::<Library>(
		r#"{
  "lists": [
    {
      "name": "Watching",
      "isCustomList": false,
      "status": "CURRENT",
      "entries": [{
        "id": 7,
        "status": "CURRENT",
        "progress": 3,
        "score": 88,
        "priority": 2,
        "media": {
          "id": 154587,
          "idMal": 52991,
          "isAdult": false,
          "title": {"userPreferred": "Frieren", "romaji": null, "english": null, "native": null},
          "nextAiringEpisode": {"airingAt": 1787740000, "episode": 4}
        }
      }]
    },
    {"name": "Rewatch someday", "isCustomList": true, "status": null, "entries": []}
  ]
}"#,
	)
	.unwrap();

	assert_eq!(
		library,
		Library {
			lists: vec![
				ListGroup {
					name: "Watching".into(),
					is_custom_list: false,
					status: Some(ListStatus::Current),
					entries: vec![ListEntry {
						id: 7,
						status: Some(ListStatus::Current),
						progress: 3,
						score: 88.0,
						priority: 2,
						media: Media {
							id: 154_587,
							id_mal: Some(52_991),
							is_adult: Some(false),
							title: MediaTitle {
								user_preferred: Some("Frieren".into()),
								romaji: None,
								english: None,
								native: None,
							},
							next_airing_episode: Some(NextAiring {
								airing_at: 1_787_740_000,
								episode: 4,
							}),
						},
					}],
				},
				ListGroup {
					name: "Rewatch someday".into(),
					is_custom_list: true,
					status: None,
					entries: Vec::new(),
				},
			],
		},
	);
}

#[tokio::test]
async fn read_only_client_rejects_mutations_before_network_access() {
	let error = super::Client::public()
		.unwrap()
		.query::<serde_json::Value, _>(
			"mutation UpdateProgress { SaveMediaListEntry { id } }",
			serde_json::json!({}),
		)
		.await
		.unwrap_err();

	assert_eq!(
		error,
		crate::error::Error::new(
			"a365's AniList integration is read-only and refuses mutations.",
		),
	);
}

#[test]
fn adult_library_filtering_hides_true_and_unclassified_entries() {
	let entry = |id, is_adult| super::ListEntry {
		id,
		status: None,
		progress: 0,
		score: 0.0,
		priority: 0,
		media: super::Media {
			id,
			id_mal: None,
			is_adult,
			title: MediaTitle {
				user_preferred: Some(format!("Series {id}")),
				romaji: None,
				english: None,
				native: None,
			},
			next_airing_episode: None,
		},
	};
	let mut library = Library {
		lists: vec![super::ListGroup {
			name: "All".into(),
			is_custom_list: false,
			status: None,
			entries: vec![
				entry(1, Some(false)),
				entry(2, Some(true)),
				entry(3, None),
			],
		}],
	};

	filter_adult(&mut library, AdultContent::Hidden);

	assert_eq!(
		library.lists[0]
			.entries
			.iter()
			.map(|entry| entry.id)
			.collect::<Vec<_>>(),
		vec![1],
	);
}

#[test]
fn jwt_expiry_is_decoded_without_exposing_the_token() {
	let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":1787740000}"#);
	let token = format!("header.{payload}.signature");

	assert_eq!(token_expiry(&token), Some("2026-08-26 10:26 UTC".into()),);
}
