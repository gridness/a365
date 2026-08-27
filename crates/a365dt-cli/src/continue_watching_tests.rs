use pretty_assertions::assert_eq;

use super::{Entry, Store};
use crate::{
	api::Episode,
	content::{ContentSource, SeriesKey},
	playback::Position,
	select::TrackKey,
};

fn entry(episode_id: u64) -> Entry {
	Entry {
		series: SeriesKey::new(ContentSource::Anime365, 7),
		series_title: "Frieren".into(),
		episode_id,
		episode_label: episode_id.to_string(),
		track: TrackKey {
			kind: "sub".into(),
			language: "ru".into(),
			authors: "Team".into(),
		},
		height: 1080,
		position: Position::START,
	}
}

#[test]
fn advancing_to_the_next_episode_resets_its_position() {
	let next = Episode {
		source: ContentSource::Anime365,
		id: 4,
		episode_int: "4".into(),
		episode_full: "4".into(),
	};

	assert_eq!(
		entry(3)
			.with_position(Position::from_seconds(754))
			.with_episode(&next),
		entry(4),
	);
}

#[tokio::test]
async fn state_round_trips_atomically_and_clear_is_idempotent() {
	let directory = std::env::temp_dir().join(format!(
		"a365-continue-test-{}",
		uuid::Uuid::now_v7().simple()
	));
	tokio::fs::create_dir(&directory).await.unwrap();
	let store = Store::at(&directory);

	assert_eq!(store.load().await.unwrap(), None);
	store.save(&entry(3)).await.unwrap();
	assert_eq!(store.load().await.unwrap(), Some(entry(3)));
	let resumed = entry(4).with_position(Position::from_seconds(12 * 60 + 34));
	store.save(&resumed).await.unwrap();
	assert_eq!(store.load().await.unwrap(), Some(resumed));
	store.clear().await.unwrap();
	store.clear().await.unwrap();
	assert_eq!(store.load().await.unwrap(), None);

	tokio::fs::remove_dir(directory).await.unwrap();
}

#[tokio::test]
async fn old_state_without_a_position_starts_from_the_beginning() {
	let directory = std::env::temp_dir().join(format!(
		"a365-continue-compat-test-{}",
		uuid::Uuid::now_v7().simple()
	));
	tokio::fs::create_dir(&directory).await.unwrap();
	tokio::fs::write(
		directory.join(super::FILE_NAME),
		r#"{
  "version": 1,
  "entry": {
    "series": { "source": "anime365", "id": 7 },
    "series_title": "Frieren",
    "episode_id": 3,
    "episode_label": "3",
    "track": { "kind": "sub", "language": "ru", "authors": "Team" },
    "height": 1080
  }
}"#,
	)
	.await
	.unwrap();

	assert_eq!(Store::at(&directory).load().await.unwrap(), Some(entry(3)));

	tokio::fs::remove_dir_all(directory).await.unwrap();
}
