use std::{fs, process, time::SystemTime};

use pretty_assertions::assert_eq;

use super::{
	CompletedRelease, Inspection, MigrationPreparation, RebuildPermission,
	Release, ReleaseState, Store, prepare_migration_at, prune_at,
};
use crate::{
	api::{Episode, Series},
	content::ContentSource,
	telemetry::Recorder,
};

fn temporary_directory(name: &str) -> std::path::PathBuf {
	std::env::temp_dir().join(format!(
		"a365-{name}-{}-{}",
		process::id(),
		SystemTime::now()
			.duration_since(SystemTime::UNIX_EPOCH)
			.unwrap()
			.as_nanos()
	))
}

fn series(id: u64, title: &str) -> Series {
	Series {
		source: ContentSource::Anime365,
		id,
		title: title.into(),
		year: Some(2020),
		type_title: Some("TV".into()),
		number_of_episodes: Some(24),
		my_anime_list_id: Some(52_991),
		anilist_id: Some(154_587),
		poster_url_small: Some("https://example.com/poster.jpg".into()),
		episodes: vec![Episode {
			source: ContentSource::Anime365,
			id: 70,
			episode_int: "1".into(),
			episode_full: "Episode 1".into(),
		}],
	}
}

#[tokio::test]
async fn configures_the_cache_connection_for_bounded_local_writes() {
	let directory = temporary_directory("cache-settings");
	let store = Store::at(directory.clone()).await;
	let pool = &store.available.as_ref().unwrap().pool;

	assert_eq!(
		(
			sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
				.fetch_one(pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
				.fetch_one(pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
				.fetch_one(pool)
				.await
				.unwrap(),
			sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
				.fetch_one(pool)
				.await
				.unwrap(),
		),
		("wal".into(), 1, 1, 5_000)
	);

	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn stores_the_catalogue_projection_without_episode_or_poster_details() {
	let directory = temporary_directory("cache-storage");
	let store = Store::at(directory.clone()).await;
	let stored = series(7, "Магическая битва");
	let mut expected = stored.clone();
	expected.poster_url_small = None;
	expected.episodes.clear();
	let mut stored_h365 = series(7, "H365 title");
	stored_h365.source = ContentSource::H365;
	let mut expected_h365 = stored_h365.clone();
	expected_h365.poster_url_small = None;
	expected_h365.episodes.clear();
	let (_, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.commit_refresh(ContentSource::Anime365, vec![stored]);
	writer.commit_refresh(ContentSource::H365, vec![stored_h365.clone()]);
	writer.remember_alias("hidden alias".into(), stored_h365);
	writer.finish().await.unwrap();
	assert!(directory.join("cache.sqlite").exists());

	let (mut loaded, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.finish().await.unwrap();
	let suggestions =
		loaded.suggestions("hidden alias", &[], &Recorder::default());
	assert_eq!(
		(0..suggestions.matches().len())
			.filter_map(|position| suggestions.series(position))
			.cloned()
			.collect::<Vec<_>>(),
		vec![expected_h365.clone()]
	);
	let suggestions = loaded.suggestions("", &[], &Recorder::default());
	assert_eq!(
		(0..suggestions.matches().len())
			.filter_map(|position| suggestions.series(position))
			.cloned()
			.collect::<Vec<_>>(),
		vec![expected, expected_h365]
	);
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn distinguishes_discovered_series_from_a_refreshed_catalogue() {
	let directory = temporary_directory("cache-discovery");
	let path = directory.join("cache.sqlite");
	let store = Store::at(directory.clone()).await;
	let (_, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.discover(vec![series(1, "Discovered")]);
	writer.finish().await.unwrap();

	let inspection = store.inspect().await;
	let Inspection::Unrefreshed {
		path: actual_path,
		series,
		..
	} = inspection
	else {
		panic!("expected an unrefreshed cache, got {inspection:?}");
	};
	assert_eq!((actual_path, series), (path, 1));

	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn refresh_skips_untitled_series_without_rolling_back_the_catalogue() {
	let directory = temporary_directory("cache-untitled-series");
	let store = Store::at(directory.clone()).await;
	let (_, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.commit_refresh(
		ContentSource::Anime365,
		vec![series(1, ""), series(2, "Cacheable")],
	);
	writer.finish().await.unwrap();

	let inspection = store.inspect().await;
	let Inspection::Ready { series, .. } = inspection else {
		panic!("expected a refreshed cache, got {inspection:?}");
	};
	assert_eq!(series, 1);

	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn retains_the_latest_completed_release() {
	let directory = temporary_directory("release-storage");
	let store = Store::at(directory.clone()).await;
	let expected = Release {
		tag_name: "v1.2.3".into(),
		html_url: "https://example.com/release".into(),
	};
	let older = Release {
		tag_name: "v1.2.2".into(),
		html_url: "https://example.com/older".into(),
	};
	let completed = CompletedRelease::now(expected.clone());
	let completed_at_ms = completed.completed_at_ms;

	store.save_release(completed).await.unwrap();
	store
		.save_release(CompletedRelease {
			release: older,
			completed_at_ms: completed_at_ms.saturating_sub(1),
		})
		.await
		.unwrap();

	assert_eq!(
		store.load_release().await.unwrap(),
		ReleaseState::Fresh(expected)
	);
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn treats_future_release_timestamps_as_stale() {
	let directory = temporary_directory("future-release");
	let store = Store::at(directory.clone()).await;
	let release = Release {
		tag_name: "v1.2.3".into(),
		html_url: "https://example.com/release".into(),
	};
	store
		.save_release(CompletedRelease {
			release: release.clone(),
			completed_at_ms: super::now_ms().saturating_add(60_000),
		})
		.await
		.unwrap();

	assert_eq!(
		store.load_release().await.unwrap(),
		ReleaseState::Stale(release)
	);
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn prunes_cache_idempotently_without_removing_the_database() {
	let directory = temporary_directory("cache-prune");
	fs::create_dir_all(&directory).unwrap();
	let store = Store::at(directory.clone()).await;
	let (_, writer) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());
	writer.commit_refresh(ContentSource::Anime365, vec![series(1, "Cached")]);
	writer.finish().await.unwrap();
	store.close().await;

	prune_at(&directory, RebuildPermission::Preauthorized)
		.await
		.unwrap();
	prune_at(&directory, RebuildPermission::Preauthorized)
		.await
		.unwrap();

	assert!(directory.join("cache.sqlite").exists());
	let store = Store::at(directory.clone()).await;
	assert!(matches!(
		store.inspect().await,
		super::Inspection::Missing { .. }
	));
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn prune_blocks_a_refresh_that_started_before_it() {
	let directory = temporary_directory("cache-prune-barrier");
	let store = Store::at(directory.clone()).await;
	let (_, stale_refresh) = store
		.load_catalogue()
		.await
		.unwrap()
		.into_session(&store, Recorder::default());

	prune_at(&directory, RebuildPermission::Preauthorized)
		.await
		.unwrap();
	stale_refresh
		.commit_refresh(ContentSource::Anime365, vec![series(1, "Stale")]);
	stale_refresh.finish().await.unwrap();

	assert!(matches!(
		store.inspect().await,
		super::Inspection::Missing { .. }
	));
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn damaged_storage_requires_authorization_before_rebuild() {
	let directory = temporary_directory("cache-rebuild");
	fs::create_dir_all(&directory).unwrap();
	let path = directory.join("cache.sqlite");
	fs::write(&path, b"damaged").unwrap();

	let error = prune_at(&directory, RebuildPermission::Ask)
		.await
		.unwrap_err();
	assert!(error.to_string().contains("cache prune --yes"));
	assert_eq!(fs::read(&path).unwrap(), b"damaged");

	assert_eq!(
		prepare_migration_at(&directory).await.unwrap(),
		MigrationPreparation::Rebuilt,
	);
	let store = Store::at(directory.clone()).await;
	assert!(matches!(
		store.inspect().await,
		super::Inspection::Missing { .. }
	));
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn damaged_schema_requires_authorization_before_rebuild() {
	let directory = temporary_directory("cache-schema-rebuild");
	let store = Store::at(directory.clone()).await;
	sqlx::query("DROP TABLE series")
		.execute(&store.available.as_ref().unwrap().pool)
		.await
		.unwrap();
	store.close().await;

	let error = prune_at(&directory, RebuildPermission::Ask)
		.await
		.unwrap_err();
	assert!(error.to_string().contains("cache prune --yes"));

	prune_at(&directory, RebuildPermission::Preauthorized)
		.await
		.unwrap();
	let store = Store::at(directory.clone()).await;
	assert!(matches!(
		store.inspect().await,
		super::Inspection::Missing { .. }
	));
	store.close().await;
	fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn retires_legacy_files_only_after_successful_initialization() {
	let directory = temporary_directory("cache-cutover");
	fs::create_dir_all(&directory).unwrap();
	for file in ["series.json", "latest-release.json"] {
		fs::write(directory.join(file), b"legacy").unwrap();
	}
	let store = Store::at(directory.clone()).await;
	for file in ["series.json", "latest-release.json"] {
		assert!(!directory.join(file).exists());
	}
	store.close().await;
	fs::remove_dir_all(&directory).unwrap();

	let failed_directory = temporary_directory("cache-cutover-failed");
	fs::create_dir_all(failed_directory.join("cache.sqlite")).unwrap();
	fs::write(failed_directory.join("series.json"), b"legacy").unwrap();
	let store = Store::at(failed_directory.clone()).await;
	assert!(store.initialization_warning().is_some());
	assert!(failed_directory.join("series.json").exists());
	store.close().await;
	fs::remove_dir_all(failed_directory).unwrap();
}
