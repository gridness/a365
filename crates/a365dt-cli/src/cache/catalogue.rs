use std::{
	collections::{BTreeMap, HashMap, HashSet},
	sync::Arc,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
	api::Series,
	content::{ContentSource, SeriesKey},
	search::{Search, normalize_query},
	telemetry::{CatalogueUse, Operation, Recorder},
};

pub(crate) const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

type Row = [String; 4];

#[derive(Debug)]
struct Index {
	rows: Vec<Row>,
	search: Search,
}

#[derive(Debug, Default)]
pub(crate) struct Catalogue {
	pub(super) refreshed_at: BTreeMap<ContentSource, u64>,
	pub(super) series: Vec<Series>,
	pub(super) aliases: BTreeMap<String, SeriesKey>,
	started_with: HashSet<SeriesKey>,
	index: Option<Arc<Index>>,
}

impl Clone for Catalogue {
	fn clone(&self) -> Self {
		Self {
			refreshed_at: self.refreshed_at.clone(),
			series: self.series.clone(),
			aliases: self.aliases.clone(),
			started_with: self.started_with.clone(),
			index: self.index.clone(),
		}
	}
}

pub(crate) struct Suggestions<'a> {
	series: &'a [Series],
	rows: &'a [Row],
	matches: Vec<usize>,
}

impl Catalogue {
	pub fn new(series: Vec<Series>) -> Self {
		let mut catalogue = Self::default();
		catalogue.upsert(series);
		catalogue.started_with = catalogue.ids();
		catalogue
	}

	#[cfg(test)]
	pub fn refreshed(series: Vec<Series>) -> Self {
		let sources = series
			.iter()
			.map(|series| series.source)
			.collect::<HashSet<_>>();
		let mut seen = HashSet::new();
		let series = series
			.into_iter()
			.filter(|series| seen.insert(series.key()))
			.collect();
		let mut catalogue = Self::new(series);
		let refreshed_at = now();
		catalogue.refreshed_at = sources
			.into_iter()
			.map(|source| (source, refreshed_at))
			.collect();
		catalogue
	}

	pub fn refreshed_source(
		source: ContentSource,
		series: Vec<Series>,
	) -> Self {
		let mut catalogue = Self::new(series);
		catalogue.refreshed_at.insert(source, now());
		catalogue
	}

	pub fn is_fresh_for(&self, sources: &HashSet<ContentSource>) -> bool {
		self.is_fresh_for_at(sources, now())
	}

	fn is_fresh_for_at(
		&self,
		sources: &HashSet<ContentSource>,
		now: u64,
	) -> bool {
		sources.iter().all(|source| {
			self.refreshed_at.get(source).is_some_and(|refreshed_at| {
				now.saturating_sub(*refreshed_at) < MAX_AGE.as_secs()
			})
		})
	}

	pub fn is_empty(&self) -> bool {
		self.series.is_empty()
	}

	pub fn retain_sources(&mut self, sources: &HashSet<ContentSource>) {
		self.refreshed_at
			.retain(|source, _| sources.contains(source));
		self.series
			.retain(|series| sources.contains(&series.source));
		self.aliases.retain(|_, key| sources.contains(&key.source));
		self.started_with
			.retain(|key| sources.contains(&key.source));
		self.index = None;
	}

	pub fn remove_source(&mut self, source: ContentSource) {
		self.refreshed_at.remove(&source);
		self.series.retain(|series| series.source != source);
		self.aliases.retain(|_, key| key.source != source);
		self.started_with.retain(|key| key.source != source);
		self.index = None;
	}

	pub fn series(&self, row: usize) -> &Series {
		&self.series[row]
	}

	pub fn row_of(&self, key: SeriesKey) -> Option<usize> {
		self.series.iter().position(|series| series.key() == key)
	}

	pub(crate) fn external_match(
		&self,
		my_anime_list_id: Option<u64>,
		anilist_id: u64,
		sources: &HashSet<ContentSource>,
	) -> Option<&Series> {
		let enabled = |series: &&Series| sources.contains(&series.source);
		my_anime_list_id
			.and_then(|id| {
				self.series
					.iter()
					.filter(enabled)
					.find(|series| series.my_anime_list_id == Some(id))
			})
			.or_else(|| {
				self.series
					.iter()
					.filter(enabled)
					.find(|series| series.anilist_id == Some(anilist_id))
			})
	}

	pub fn suggestions<'a>(
		&'a mut self,
		query: &str,
		server_matches: &[SeriesKey],
		telemetry: &Recorder,
	) -> Suggestions<'a> {
		self.suggestions_to(query, server_matches, telemetry, None)
	}

	pub(crate) fn limited_suggestions<'a>(
		&'a mut self,
		query: &str,
		server_matches: &[SeriesKey],
		telemetry: &Recorder,
		limit: usize,
	) -> Suggestions<'a> {
		self.suggestions_to(query, server_matches, telemetry, Some(limit))
	}

	fn suggestions_to<'a>(
		&'a mut self,
		query: &str,
		server_matches: &[SeriesKey],
		telemetry: &Recorder,
		limit: Option<usize>,
	) -> Suggestions<'a> {
		let preferred = self.preferred_rows(query, server_matches);
		self.ensure_index(telemetry);
		let index = self.index.as_ref().expect("catalogue index exists");
		let _measurement =
			telemetry.measure_items(Operation::SearchRank, index.search.len());
		let ranked = limit.map_or_else(
			|| index.search.ranked(query),
			|limit| {
				index
					.search
					.ranked_limit(query, limit.saturating_add(preferred.len()))
			},
		);
		let mut seen = HashSet::new();
		let mut matches = Vec::new();
		for row in preferred.into_iter().chain(ranked) {
			if seen.insert(row) {
				matches.push(row);
				if limit.is_some_and(|limit| matches.len() == limit) {
					break;
				}
			}
		}
		Suggestions {
			series: &self.series,
			rows: &index.rows,
			matches,
		}
	}

	pub(crate) fn prepare_search(&mut self, telemetry: &Recorder) {
		self.ensure_index(telemetry);
	}

	pub fn ranked(
		series: &[Series],
		query: &str,
		limit: usize,
		telemetry: &Recorder,
	) -> Vec<Series> {
		let rows = series_rows(series);
		let measurement =
			telemetry.measure_items(Operation::SearchIndex, rows.len());
		let search = Search::new(&rows);
		drop(measurement);
		let _measurement =
			telemetry.measure_items(Operation::SearchRank, search.len());
		search
			.ranked(query)
			.into_iter()
			.take(limit)
			.map(|index| series[index].clone())
			.collect()
	}

	pub fn upsert(&mut self, incoming: Vec<Series>) {
		let mut positions = self
			.series
			.iter()
			.enumerate()
			.map(|(index, series)| (series.key(), index))
			.collect::<HashMap<_, _>>();
		for series in incoming {
			let key = series.key();
			if let Some(index) = positions.get(&key).copied() {
				self.series[index] = series;
			} else {
				positions.insert(key, self.series.len());
				self.series.push(series);
			}
		}
		self.index = None;
	}

	pub fn remember_alias(&mut self, query: &str, key: SeriesKey) {
		let query = normalize_query(query);
		if !query.is_empty() {
			self.aliases.insert(query, key);
		}
	}

	pub fn remove_series(&mut self, key: SeriesKey) {
		self.series.retain(|series| series.key() != key);
		self.aliases.retain(|_, alias| *alias != key);
		self.index = None;
	}

	pub fn merge_refresh(
		&mut self,
		mut refreshed: Self,
		preserved_series: &HashSet<SeriesKey>,
	) {
		let mut preserved_series = preserved_series.clone();
		preserved_series.extend(self.aliases.values().copied());
		let refreshed_sources = refreshed
			.refreshed_at
			.keys()
			.copied()
			.collect::<HashSet<_>>();
		let mut ids = refreshed.ids();
		refreshed.series.extend(
			self.series
				.iter()
				.filter(|series| {
					(!refreshed_sources.contains(&series.source)
						|| preserved_series.contains(&series.key()))
						&& ids.insert(series.key())
				})
				.cloned(),
		);
		refreshed.aliases.clone_from(&self.aliases);
		refreshed.aliases.retain(|_, id| ids.contains(id));
		self.refreshed_at.extend(refreshed.refreshed_at);
		self.series = refreshed.series;
		self.aliases = refreshed.aliases;
		self.index = None;
	}

	pub fn catalogue_use(&self, selected: SeriesKey) -> CatalogueUse {
		if self.started_with.contains(&selected) {
			CatalogueUse::Hit
		} else {
			CatalogueUse::Miss
		}
	}

	fn preferred_rows(
		&self,
		query: &str,
		server_matches: &[SeriesKey],
	) -> Vec<usize> {
		let mut ids = self
			.aliases
			.get(&normalize_query(query))
			.copied()
			.into_iter()
			.collect::<Vec<_>>();
		ids.extend_from_slice(server_matches);
		let mut seen = HashSet::new();
		// ponytail: at most 11 priorities; add an ID index if that limit grows.
		ids.into_iter()
			.filter(|id| seen.insert(*id))
			.filter_map(|id| self.row_of(id))
			.collect()
	}

	fn ensure_index(&mut self, telemetry: &Recorder) {
		if self.index.is_some() {
			return;
		}
		let rows = series_rows(&self.series);
		let measurement =
			telemetry.measure_items(Operation::SearchIndex, rows.len());
		let search = Search::new(&rows);
		drop(measurement);
		self.index = Some(Arc::new(Index { rows, search }));
	}

	fn ids(&self) -> HashSet<SeriesKey> {
		self.series.iter().map(Series::key).collect()
	}
}

impl Suggestions<'_> {
	pub fn is_empty(&self) -> bool {
		self.matches.is_empty()
	}

	pub fn rows(&self) -> &[Row] {
		self.rows
	}

	pub fn matches(&self) -> &[usize] {
		&self.matches
	}

	pub fn matching_rows(&self, limit: usize) -> Vec<Row> {
		self.matches
			.iter()
			.take(limit)
			.map(|index| self.rows[*index].clone())
			.collect()
	}

	pub fn series(&self, position: usize) -> Option<&Series> {
		self.matches.get(position).map(|index| &self.series[*index])
	}
}

impl Catalogue {
	pub(super) fn from_parts(
		refreshed_at: BTreeMap<ContentSource, u64>,
		series: Vec<Series>,
		aliases: BTreeMap<String, SeriesKey>,
	) -> Self {
		let started_with = series.iter().map(Series::key).collect();
		Self {
			refreshed_at,
			series,
			aliases,
			started_with,
			index: None,
		}
	}
}

fn series_rows(series: &[Series]) -> Vec<Row> {
	series
		.iter()
		.map(|item| {
			[
				item.title.clone(),
				item.year
					.map_or_else(|| "?".into(), |year| year.to_string()),
				format!(
					"{} · {}",
					item.source,
					item.type_title.as_deref().unwrap_or("Unknown type")
				),
				format!(
					"{} episodes",
					item.number_of_episodes
						.map_or_else(|| "?".into(), |count| count.to_string())
				),
			]
		})
		.collect()
}

fn now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;
