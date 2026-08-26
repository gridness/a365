use std::collections::HashSet;

use pretty_assertions::assert_eq;

use super::{Catalogue, MAX_AGE};
use crate::{
	api::Series,
	content::{ContentSource, SeriesKey},
	telemetry::{CatalogueUse, Recorder},
};

fn series(id: u64, title: &str, year: u16) -> Series {
	Series {
		source: ContentSource::Anime365,
		id,
		title: title.into(),
		year: Some(year),
		type_title: Some("TV".into()),
		number_of_episodes: Some(24),
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: None,
		episodes: Vec::new(),
	}
}

fn h365_series(id: u64, title: &str, year: u16) -> Series {
	let mut series = series(id, title, year);
	series.source = ContentSource::H365;
	series
}

fn matching_series(
	catalogue: &mut Catalogue,
	query: &str,
	server_matches: &[SeriesKey],
) -> Vec<Series> {
	let suggestions =
		catalogue.suggestions(query, server_matches, &Recorder::default());
	(0..suggestions.matches().len())
		.filter_map(|position| suggestions.series(position))
		.cloned()
		.collect()
}

#[test]
fn expires_catalogue_after_one_day() {
	let mut catalogue = Catalogue::default();
	catalogue.refreshed_at.insert(ContentSource::Anime365, 0);
	let sources = HashSet::from([ContentSource::Anime365]);

	assert!(catalogue.is_fresh_for_at(&sources, MAX_AGE.as_secs() - 1));
	assert!(!catalogue.is_fresh_for_at(&sources, MAX_AGE.as_secs()));
}

#[test]
fn updates_existing_series_and_deduplicates_new_results() {
	let mut catalogue = Catalogue::new(vec![series(1, "Old title", 2020)]);
	let expected = vec![
		series(1, "Current title", 2021),
		series(2, "Final title", 2023),
	];

	catalogue.upsert(vec![
		expected[0].clone(),
		series(2, "Superseded title", 2022),
		expected[1].clone(),
	]);

	assert_eq!(matching_series(&mut catalogue, "", &[]), expected);
}

#[test]
fn keeps_equal_remote_ids_from_different_sources_distinct() {
	let anime365 = series(1, "Anime365 title", 2020);
	let h365 = h365_series(1, "H365 title", 2021);
	let mut catalogue = Catalogue::new(vec![anime365.clone(), h365.clone()]);

	catalogue.remember_alias("explicit hidden alias", h365.key());

	assert_eq!(
		(
			matching_series(&mut catalogue, "", &[]),
			matching_series(&mut catalogue, "explicit hidden alias", &[]),
		),
		(vec![anime365, h365.clone()], vec![h365])
	);
}

#[test]
fn disabling_adult_availability_hides_stale_h365_rows_immediately() {
	let anime365 = series(1, "Anime365 title", 2020);
	let h365 = h365_series(1, "H365 title", 2021);
	let mut catalogue = Catalogue::new(vec![anime365.clone(), h365]);

	catalogue.retain_sources(&HashSet::from([ContentSource::Anime365]));

	assert_eq!(matching_series(&mut catalogue, "", &[]), vec![anime365]);
}

#[test]
fn refresh_freshness_is_tracked_for_each_enabled_source() {
	let now = MAX_AGE.as_secs() + 10;
	let mut catalogue = Catalogue::default();
	catalogue
		.refreshed_at
		.insert(ContentSource::Anime365, now - 1);

	assert!(
		catalogue
			.is_fresh_for_at(&HashSet::from([ContentSource::Anime365]), now,)
	);
	assert!(!catalogue.is_fresh_for_at(
		&HashSet::from([ContentSource::Anime365, ContentSource::H365]),
		now,
	));
}

#[test]
fn external_series_mapping_prefers_mal_then_anilist_and_preserves_unmatched() {
	let mut mal = series(1, "MAL match", 2020);
	mal.my_anime_list_id = Some(52_991);
	let mut anilist = h365_series(2, "AniList match", 2021);
	anilist.anilist_id = Some(154_587);
	let catalogue = Catalogue::new(vec![mal.clone(), anilist.clone()]);
	let sources = HashSet::from([ContentSource::Anime365, ContentSource::H365]);

	assert_eq!(
		(
			catalogue.external_match(Some(52_991), 999, &sources),
			catalogue.external_match(None, 154_587, &sources),
			catalogue.external_match(Some(1), 2, &sources),
		),
		(Some(&mal), Some(&anilist), None),
	);
}

#[test]
fn prioritizes_learned_and_server_suggestions_without_duplicates() {
	let expected = vec![
		series(1, "Jujutsu Kaisen", 2020),
		series(2, "Tengen Toppa Gurren Lagann", 2007),
	];
	let mut catalogue = Catalogue::new(expected.clone());

	assert_eq!(
		matching_series(&mut catalogue, "jjk", &[expected[0].key()]),
		vec![expected[0].clone()]
	);

	catalogue.remember_alias("  JJK!!!  ", expected[1].key());
	assert_eq!(
		matching_series(
			&mut catalogue,
			"jjk",
			&[expected[0].key(), expected[1].key()],
		),
		vec![expected[1].clone(), expected[0].clone()]
	);
}

#[test]
fn removes_a_series_and_its_aliases_together() {
	let remaining = series(1, "Jujutsu Kaisen", 2020);
	let mut catalogue = Catalogue::new(vec![
		remaining.clone(),
		series(2, "Tengen Toppa Gurren Lagann", 2007),
	]);
	catalogue
		.remember_alias("gurren", SeriesKey::new(ContentSource::Anime365, 2));

	catalogue.remove_series(SeriesKey::new(ContentSource::Anime365, 2));

	assert_eq!(
		(
			matching_series(&mut catalogue, "", &[]),
			matching_series(&mut catalogue, "gurren", &[]),
		),
		(vec![remaining], Vec::new())
	);
}

#[test]
fn merges_refreshes_with_current_results_and_valid_aliases() {
	let aliased = series(1, "Tengen Toppa Gurren Lagann", 2007);
	let current = series(2, "Current query result", 2024);
	let mut catalogue = Catalogue::new(vec![
		aliased.clone(),
		current.clone(),
		series(4, "Stale result", 1999),
	]);
	catalogue.remember_alias("ttgl", aliased.key());
	let refreshed = Catalogue::refreshed(vec![
		series(3, "Old refreshed title", 2020),
		series(3, "Current refreshed title", 2021),
	]);

	catalogue.merge_refresh(refreshed, &HashSet::from([current.key()]));

	assert_eq!(
		(
			matching_series(&mut catalogue, "", &[]),
			matching_series(&mut catalogue, "ttgl", &[]),
			catalogue.is_fresh_for(&HashSet::from([ContentSource::Anime365,])),
		),
		(
			vec![
				series(3, "Old refreshed title", 2020),
				aliased.clone(),
				current,
			],
			vec![aliased],
			true,
		)
	);
}

#[test]
fn ranks_series_and_classifies_catalogue_use_through_the_interface() {
	let expected = series(1, "Магическая битва", 2020);
	let mut catalogue = Catalogue::new(vec![
		expected.clone(),
		series(2, "Битва через пять секунд после встречи", 2021),
	]);
	catalogue.upsert(vec![series(3, "New result", 2024)]);
	let rows = catalogue
		.suggestions("битва магическая", &[], &Recorder::default())
		.matching_rows(10);

	assert_eq!(
		(
			matching_series(&mut catalogue, "битва магическая", &[]),
			rows,
			[
				catalogue.catalogue_use(expected.key()),
				catalogue
					.catalogue_use(SeriesKey::new(ContentSource::Anime365, 3,)),
			],
		),
		(
			vec![expected],
			vec![[
				"Магическая битва".into(),
				"2020".into(),
				"Anime365 · TV".into(),
				"24 episodes".into(),
			]],
			[CatalogueUse::Hit, CatalogueUse::Miss],
		)
	);
}
