use std::{fs, process, time::SystemTime};

use pretty_assertions::assert_eq;

use super::{Catalogue, Store};
use crate::{
	api::Series,
	content::{ContentSource, SeriesKey},
	telemetry::Recorder,
};

fn series(id: u64, title: &str) -> Series {
	Series {
		source: ContentSource::Anime365,
		id,
		title: title.into(),
		year: Some(2024),
		type_title: Some("TV".into()),
		number_of_episodes: Some(12),
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: None,
		episodes: Vec::new(),
	}
}

fn matching_series(catalogue: &mut Catalogue, query: &str) -> Vec<Series> {
	let suggestions = catalogue.suggestions(query, &[], &Recorder::default());
	(0..suggestions.matches().len())
		.filter_map(|position| suggestions.series(position))
		.cloned()
		.collect()
}

#[tokio::test]
async fn semantic_writer_drains_every_mutation_before_finishing() {
	let directory = std::env::temp_dir().join(format!(
		"a365-cache-writer-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	));
	let store = Store::at(directory.clone()).await;
	let (mut catalogue, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	let discovered = series(1, "Discovered");
	let missing = series(2, "Missing");
	let refreshed = series(3, "Refreshed");

	catalogue.upsert(vec![discovered.clone(), missing.clone()]);
	writer.discover(vec![discovered.clone(), missing]);
	catalogue.remember_alias("known", discovered.key());
	writer.remember_alias("known".into(), discovered.clone());
	catalogue.remove_series(SeriesKey::new(ContentSource::Anime365, 2));
	writer.remove_missing(SeriesKey::new(ContentSource::Anime365, 2));
	catalogue.merge_refresh(
		Catalogue::refreshed(vec![refreshed.clone()]),
		&[discovered.key()].into(),
	);
	writer.commit_refresh(ContentSource::Anime365, vec![refreshed.clone()]);
	writer.finish().await.unwrap();

	let (mut loaded, loaded_writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	loaded_writer.finish().await.unwrap();
	assert_eq!(
		(
			matching_series(&mut loaded, ""),
			matching_series(&mut loaded, "known"),
		),
		(vec![refreshed, discovered.clone()], vec![discovered],)
	);

	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn stale_removal_preserves_a_concurrent_alias_update() {
	let directory = temporary_directory("revision");
	let store = Store::at(directory.clone()).await;
	let (_, seed) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	seed.discover(vec![series(1, "Original")]);
	seed.finish().await.unwrap();

	let (_, stale) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	let concurrent_store = Store::at(directory.clone()).await;
	let (_, concurrent) = concurrent_store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&concurrent_store, Recorder::default());
	let updated = series(1, "Updated");
	concurrent.remember_alias("known".into(), updated.clone());
	concurrent.finish().await.unwrap();
	stale.remove_missing(updated.key());
	stale.finish().await.unwrap();

	let (mut loaded, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.finish().await.unwrap();
	assert_eq!(matching_series(&mut loaded, "known"), vec![updated]);

	concurrent_store.close().await;
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn refresh_preserves_newer_discoveries_and_newer_refreshes_win() {
	let directory = temporary_directory("refresh-revision");
	let store = Store::at(directory.clone()).await;
	let original = series(1, "Original");
	let (_, seed) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	seed.commit_refresh(ContentSource::Anime365, vec![original]);
	seed.finish().await.unwrap();

	let (_, stale_refresh) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	let concurrent_store = Store::at(directory.clone()).await;
	let (_, discover) = concurrent_store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&concurrent_store, Recorder::default());
	let concurrently_updated = series(1, "Concurrent update");
	let discovered = series(2, "Discovered");
	discover.discover(vec![concurrently_updated.clone(), discovered.clone()]);
	discover.finish().await.unwrap();
	stale_refresh.commit_refresh(
		ContentSource::Anime365,
		vec![series(1, "Stale refresh")],
	);
	stale_refresh.finish().await.unwrap();
	let (mut loaded, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.finish().await.unwrap();
	assert_eq!(
		matching_series(&mut loaded, ""),
		vec![concurrently_updated, discovered]
	);

	let (_, older_refresh) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	let (_, newer_refresh) = concurrent_store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&concurrent_store, Recorder::default());
	let newest = series(1, "Newest");
	newer_refresh.commit_refresh(ContentSource::Anime365, vec![newest.clone()]);
	newer_refresh.finish().await.unwrap();
	older_refresh
		.commit_refresh(ContentSource::Anime365, vec![series(1, "Older")]);
	older_refresh.finish().await.unwrap();

	let (mut loaded, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.finish().await.unwrap();
	assert_eq!(matching_series(&mut loaded, ""), vec![newest]);

	concurrent_store.close().await;
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

fn temporary_directory(name: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!(
		"a365-cache-writer-{name}-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	))
}
