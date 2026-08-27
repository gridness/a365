use std::{
	collections::{BTreeMap, HashSet},
	io,
	time::Duration,
};

use console::{Key, Term};
use tokio::{
	sync::mpsc,
	task::{JoinError, JoinHandle, JoinSet},
	time::sleep,
};

use crate::{
	api::{
		Anime365, Result as ApiResult, SERIES_PAGE_SIZE, Series,
		series_key_from_url,
	},
	cache::{Catalogue, LoadedCatalogue, Store, Writer},
	content::{ContentSource, SeriesKey},
	error::Error,
	telemetry::{CatalogueUse, Operation, Recorder},
	ui::{self, selector},
};

const REFRESH_CONCURRENCY: usize = 4;
const MAX_CATALOGUE_SIZE: usize = 100_000;
pub(crate) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);
const SEARCH_LABEL: &str = "Search title or paste Anime365 URL";

pub struct Selection {
	pub series: Series,
	pub catalogue: CatalogueUse,
}

pub async fn choose(
	apis: &[Anime365],
	store: &Store,
	prefill: String,
	telemetry: &Recorder,
) -> Result<Selection, Error> {
	if prefill.starts_with("http://") || prefill.starts_with("https://") {
		return Ok(Selection {
			series: load_url(apis, &prefill).await?,
			catalogue: CatalogueUse::Bypassed,
		});
	}
	let measurement = telemetry.measure(Operation::CacheRetrieve);
	let loaded = store.load_catalogue().await;
	drop(measurement);
	let loaded = match loaded {
		Ok(loaded) => loaded,
		Err(error) => {
			ui::warning(error);
			LoadedCatalogue::unavailable()
		}
	};
	let (mut catalogue, writer) = loaded.into_session(store, telemetry.clone());
	catalogue.retain_sources(
		&apis.iter().map(Anime365::source).collect::<HashSet<_>>(),
	);
	let result = if selector::interactive_terminal() {
		choose_interactive(apis, catalogue, &writer, prefill, telemetry).await
	} else {
		choose_line(apis, catalogue, &writer, prefill, telemetry).await
	};
	if let Err(error) = writer.finish().await {
		ui::warning(error);
	}
	result
}

async fn choose_line(
	apis: &[Anime365],
	mut catalogue: Catalogue,
	writer: &Writer,
	mut query: String,
	telemetry: &Recorder,
) -> Result<Selection, Error> {
	if query.is_empty() {
		query = ui::prompt("Search title or Anime365 catalogue URL:")?;
	}
	if query.starts_with("http://") || query.starts_with("https://") {
		return Ok(Selection {
			series: load_url(apis, &query).await?,
			catalogue: CatalogueUse::Bypassed,
		});
	}
	let local_available =
		!catalogue.suggestions(&query, &[], telemetry).is_empty();
	let mut exact_ids = Vec::new();
	match remote_search(apis, &query, telemetry).await {
		Ok(results) => {
			for warning in results.warnings {
				ui::warning(warning);
			}
			exact_ids.extend(results.exact.iter().map(Series::key));
			let mut incoming = results.exact;
			incoming.extend(results.fallback);
			writer.discover(incoming.clone());
			catalogue.upsert(incoming);
		}
		Err(error) if !local_available => return Err(error),
		Err(_) => {}
	}
	let suggestions = catalogue.suggestions(&query, &exact_ids, telemetry);
	let rows = suggestions.matching_rows(10);
	if rows.is_empty() {
		return Err("No matching Anime365 series found.".into());
	}
	let selected = suggestions
		.series(ui::choose("Search results", &rows)?)
		.cloned()
		.unwrap();
	let api = api_for_source(apis, selected.source)?;
	match api.series(selected.id).await? {
		Some(series) => {
			if exact_ids.contains(&selected.key()) {
				catalogue.remember_alias(&query, selected.key());
				writer.remember_alias(query, series.clone());
			}
			Ok(Selection {
				catalogue: catalogue.catalogue_use(series.key()),
				series,
			})
		}
		None => {
			catalogue.remove_series(selected.key());
			writer.remove_missing(selected.key());
			Err("That cached Anime365 series no longer exists.".into())
		}
	}
}

async fn choose_interactive(
	apis: &[Anime365],
	mut catalogue: Catalogue,
	writer: &Writer,
	prefill: String,
	telemetry: &Recorder,
) -> Result<Selection, Error> {
	let term = Term::buffered_stdout();
	let mut server_matches = Vec::new();
	let mut state = selector::State::from_matches(prefill, Vec::new());
	let (mut rows, matches) = catalogue_view(
		&mut catalogue,
		state.query(),
		&server_matches,
		telemetry,
	);
	state.set_matches(matches);
	let mut layout = selector::Layout::new(&term, &rows);
	let mut lines =
		selector::draw(&term, SEARCH_LABEL, &rows, &mut layout, &mut state)
			.map_err(selector::term_error)?;
	let (updates_tx, mut updates) = mpsc::unbounded_channel();
	let enabled_sources =
		apis.iter().map(Anime365::source).collect::<HashSet<_>>();
	if !catalogue.is_fresh_for(&enabled_sources) {
		drop(tokio::spawn(refresh(apis.to_vec(), updates_tx.clone())));
	}
	let mut search_task = schedule_search(apis, &updates_tx, &state, telemetry);
	let mut key_task = read_key(&term);
	let mut query_results = HashSet::new();

	loop {
		let event = tokio::select! {
			key = &mut key_task => Event::Key(key),
			update = updates.recv() => Event::Update(update),
		};
		match event {
			Event::Key(key) => {
				let key = match resolve_key(key) {
					Ok(key) => key,
					Err(error) => {
						selector::clear(&term, lines)
							.map_err(selector::term_error)?;
						term.flush().map_err(selector::term_error)?;
						return Err(error);
					}
				};
				if matches!(key, Key::Enter)
					&& (state.query().starts_with("http://")
						|| state.query().starts_with("https://"))
				{
					selector::clear(&term, lines)
						.map_err(selector::term_error)?;
					term.flush().map_err(selector::term_error)?;
					return Ok(Selection {
						series: load_url(apis, state.query()).await?,
						catalogue: CatalogueUse::Bypassed,
					});
				}
				let visible = selector::visible_rows(&term);
				match state.handle(key, visible) {
					selector::Action::Selected(index) => {
						let selected = catalogue.series(index).clone();
						let api = api_for_source(apis, selected.source)?;
						let query = state.query().to_owned();
						let confirmed =
							server_matches.contains(&selected.key());
						selector::clear(&term, lines)
							.map_err(selector::term_error)?;
						term.flush().map_err(selector::term_error)?;
						let spinner = ui::spinner("Loading title…");
						let series = api.series(selected.id).await;
						spinner.finish_and_clear();
						match series? {
							Some(series) => {
								if confirmed {
									catalogue
										.remember_alias(&query, selected.key());
									writer
										.remember_alias(query, series.clone());
								}
								selector::write_choice(
									&term,
									SEARCH_LABEL,
									&rows,
									index,
								)
								.map_err(selector::term_error)?;
								return Ok(Selection {
									catalogue: catalogue
										.catalogue_use(series.key()),
									series,
								});
							}
							None => {
								catalogue.remove_series(selected.key());
								writer.remove_missing(selected.key());
								let view = catalogue_view(
									&mut catalogue,
									state.query(),
									&server_matches,
									telemetry,
								);
								rows = view.0;
								layout.replace(&term, &rows);
								state.replace_matches(view.1);
								ui::warning(
									"That cached title no longer exists; removed it.",
								);
								key_task = read_key(&term);
								lines = selector::draw(
									&term,
									SEARCH_LABEL,
									&rows,
									&mut layout,
									&mut state,
								)
								.map_err(selector::term_error)?;
								continue;
							}
						}
					}
					selector::Action::Changed => {
						server_matches.clear();
						let matches = suggestion_matches(
							&mut catalogue,
							state.query(),
							&server_matches,
							telemetry,
						);
						state.set_matches(matches);
						if let Some(task) = search_task.take() {
							task.abort();
						}
						search_task = schedule_search(
							apis,
							&updates_tx,
							&state,
							telemetry,
						);
					}
					selector::Action::Cancelled => {
						selector::clear(&term, lines)
							.map_err(selector::term_error)?;
						term.flush().map_err(selector::term_error)?;
						return Err("Cancelled.".into());
					}
					selector::Action::Continue => {}
				}
				key_task = read_key(&term);
			}
			Event::Update(Some(Update::Search(query, result)))
				if query == state.query().trim() =>
			{
				match result {
					Ok(results) => {
						let warnings = results.warnings;
						server_matches =
							results.exact.iter().map(Series::key).collect();
						let mut incoming = results.exact;
						incoming.extend(results.fallback);
						query_results.extend(incoming.iter().map(Series::key));
						if !incoming.is_empty() {
							writer.discover(incoming.clone());
							catalogue.upsert(incoming);
							let view = catalogue_view(
								&mut catalogue,
								state.query(),
								&server_matches,
								telemetry,
							);
							rows = view.0;
							layout.replace(&term, &rows);
							state.replace_matches(view.1);
						} else {
							state.replace_matches(suggestion_matches(
								&mut catalogue,
								state.query(),
								&server_matches,
								telemetry,
							));
						}
						state.select_first();
						if !warnings.is_empty() {
							selector::clear(&term, lines)
								.map_err(selector::term_error)?;
							term.flush().map_err(selector::term_error)?;
							for warning in warnings {
								ui::warning(warning);
							}
							lines = selector::draw(
								&term,
								SEARCH_LABEL,
								&rows,
								&mut layout,
								&mut state,
							)
							.map_err(selector::term_error)?;
							continue;
						}
					}
					Err(error) if !state.has_matches() => {
						selector::clear(&term, lines)
							.map_err(selector::term_error)?;
						term.flush().map_err(selector::term_error)?;
						ui::warning(error);
						lines = selector::draw(
							&term,
							SEARCH_LABEL,
							&rows,
							&mut layout,
							&mut state,
						)
						.map_err(selector::term_error)?;
						continue;
					}
					Err(_) => {}
				}
			}
			Event::Update(Some(Update::Page(offset, series))) => {
				let query = state.query().trim();
				if (offset == 0 && catalogue.is_empty())
					|| (!query.is_empty()
						&& !Catalogue::ranked(&series, query, 1, telemetry)
							.is_empty())
				{
					catalogue.upsert(series);
					let view = catalogue_view(
						&mut catalogue,
						state.query(),
						&server_matches,
						telemetry,
					);
					rows = view.0;
					layout.replace(&term, &rows);
					state.replace_matches(view.1);
				}
			}
			Event::Update(Some(Update::Refreshed(source, series))) => {
				let selected = state
					.selected_row()
					.map(|index| catalogue.series(index).key());
				writer.commit_refresh(source, series.clone());
				let refreshed = Catalogue::refreshed_source(source, series);
				catalogue.merge_refresh(refreshed, &query_results);
				let view = catalogue_view(
					&mut catalogue,
					state.query(),
					&server_matches,
					telemetry,
				);
				rows = view.0;
				layout.replace(&term, &rows);
				state.replace_matches(view.1);
				if let Some(selected) = selected {
					if let Some(row) = catalogue.row_of(selected) {
						state.select_row(row);
					} else {
						state.select_first();
					}
				}
			}
			Event::Update(Some(Update::RefreshFailed(source, error))) => {
				if source == ContentSource::H365 {
					catalogue.remove_source(source);
					let view = catalogue_view(
						&mut catalogue,
						state.query(),
						&server_matches,
						telemetry,
					);
					rows = view.0;
					layout.replace(&term, &rows);
					state.replace_matches(view.1);
				}
				selector::clear(&term, lines).map_err(selector::term_error)?;
				term.flush().map_err(selector::term_error)?;
				ui::warning(error);
				lines = selector::draw(
					&term,
					SEARCH_LABEL,
					&rows,
					&mut layout,
					&mut state,
				)
				.map_err(selector::term_error)?;
				continue;
			}
			Event::Update(Some(Update::Search(_, _))) | Event::Update(None) => {
			}
		}
		selector::clear(&term, lines).map_err(selector::term_error)?;
		lines =
			selector::draw(&term, SEARCH_LABEL, &rows, &mut layout, &mut state)
				.map_err(selector::term_error)?;
	}
}

enum Event {
	Key(Result<io::Result<Key>, JoinError>),
	Update(Option<Update>),
}

enum Update {
	Search(String, ApiResult<RemoteResults>),
	Page(usize, Vec<Series>),
	Refreshed(ContentSource, Vec<Series>),
	RefreshFailed(ContentSource, Error),
}

pub(crate) struct RemoteResults {
	pub(crate) exact: Vec<Series>,
	pub(crate) fallback: Vec<Series>,
	pub(crate) warnings: Vec<Error>,
}

fn schedule_search(
	apis: &[Anime365],
	updates: &mpsc::UnboundedSender<Update>,
	state: &selector::State,
	telemetry: &Recorder,
) -> Option<JoinHandle<()>> {
	let query = state.query().trim();
	if query.is_empty() {
		return None;
	}
	let query = query.to_owned();
	let apis = apis.to_vec();
	let updates = updates.clone();
	let telemetry = telemetry.clone();
	Some(tokio::spawn(async move {
		sleep(SEARCH_DEBOUNCE).await;
		let result = remote_search(&apis, &query, &telemetry).await;
		let _ = updates.send(Update::Search(query, result));
	}))
}

pub(crate) async fn remote_search(
	apis: &[Anime365],
	query: &str,
	telemetry: &Recorder,
) -> ApiResult<RemoteResults> {
	if series_key_from_url(query).is_some() {
		return Ok(RemoteResults {
			exact: vec![load_url(apis, query).await?],
			fallback: Vec::new(),
			warnings: Vec::new(),
		});
	}
	let mut exact = Vec::new();
	let mut fallback = Vec::new();
	let mut warnings = Vec::new();
	let mut first_error = None;
	for api in apis {
		match remote_search_source(api, query, telemetry).await {
			Ok(results) => {
				exact.extend(results.exact);
				fallback.extend(results.fallback);
			}
			Err(error) if api.source() == ContentSource::H365 => {
				warnings.push(error);
			}
			Err(error) => first_error = Some(error),
		}
	}
	if exact.is_empty()
		&& fallback.is_empty()
		&& let Some(error) = first_error
	{
		return Err(error);
	}
	Ok(RemoteResults {
		exact,
		fallback,
		warnings,
	})
}

async fn remote_search_source(
	api: &Anime365,
	query: &str,
	telemetry: &Recorder,
) -> ApiResult<RemoteResults> {
	let exact = api.search(query).await?;
	if !exact.is_empty() {
		return Ok(RemoteResults {
			exact,
			fallback: Vec::new(),
			warnings: Vec::new(),
		});
	}
	let fallback_query = api_query(query);
	let fallback = if fallback_query == query {
		Vec::new()
	} else {
		Catalogue::ranked(
			&api.search(fallback_query).await?,
			query,
			10,
			telemetry,
		)
	};
	Ok(RemoteResults {
		exact,
		fallback,
		warnings: Vec::new(),
	})
}

async fn refresh(apis: Vec<Anime365>, updates: mpsc::UnboundedSender<Update>) {
	for api in apis {
		let source = api.source();
		let page_updates = updates.clone();
		match refresh_source(api, move |offset, page| {
			let _ = page_updates.send(Update::Page(offset, page.to_vec()));
		})
		.await
		{
			Ok(series) => {
				let _ = updates.send(Update::Refreshed(source, series));
			}
			Err(error) => {
				let _ = updates.send(Update::RefreshFailed(source, error));
			}
		}
	}
}

pub(crate) async fn refresh_source(
	api: Anime365,
	on_page: impl Fn(usize, &[Series]),
) -> ApiResult<Vec<Series>> {
	let mut active = JoinSet::new();
	let mut next_offset = 0;
	for _ in 0..REFRESH_CONCURRENCY {
		spawn_page(&mut active, &api, next_offset);
		next_offset += SERIES_PAGE_SIZE;
	}
	let mut pages = BTreeMap::new();
	let mut reached_end = false;
	while let Some(joined) = active.join_next().await {
		let (offset, page) = joined.map_err(|error| {
			Error::with_debug(
				format!("The {} catalogue refresh stopped.", api.source()),
				error,
			)
		})?;
		let page = page?;
		let full = page.len() == SERIES_PAGE_SIZE;
		on_page(offset, &page);
		pages.insert(offset, page);
		if full && !reached_end {
			if next_offset >= MAX_CATALOGUE_SIZE {
				return Err(Error::new(format!(
					"The {} catalogue exceeded its safe size limit.",
					api.source()
				)));
			}
			spawn_page(&mut active, &api, next_offset);
			next_offset += SERIES_PAGE_SIZE;
		} else {
			reached_end = true;
		}
	}
	Ok(pages.into_values().flatten().collect())
}

fn spawn_page(
	active: &mut JoinSet<(usize, ApiResult<Vec<Series>>)>,
	api: &Anime365,
	offset: usize,
) {
	let api = api.clone();
	active.spawn(async move { (offset, api.series_page(offset).await) });
}

fn read_key(term: &Term) -> JoinHandle<io::Result<Key>> {
	let term = term.clone();
	tokio::task::spawn_blocking(move || term.read_key_raw())
}

fn resolve_key(key: Result<io::Result<Key>, JoinError>) -> Result<Key, Error> {
	key.map_err(|error| {
		Error::with_debug("The terminal input task stopped.", error)
	})?
	.map_err(selector::term_error)
}

fn api_query(query: &str) -> &str {
	query
		.split_whitespace()
		.max_by_key(|word| word.chars().count())
		.unwrap_or(query)
}

async fn load_url(apis: &[Anime365], input: &str) -> Result<Series, Error> {
	let key = series_key_from_url(input).ok_or_else(|| {
		"Enter an official Anime365 or H365 series catalogue URL.".to_owned()
	})?;
	let api = api_for_source(apis, key.source)?;
	api.series(key.id).await?.ok_or_else(|| {
		format!("That {} series no longer exists.", key.source).into()
	})
}

fn api_for_source(
	apis: &[Anime365],
	source: ContentSource,
) -> Result<&Anime365, Error> {
	apis.iter()
		.find(|api| api.source() == source)
		.ok_or_else(|| {
			Error::new(format!("{source} is not enabled for this invocation."))
		})
}

fn catalogue_view(
	catalogue: &mut Catalogue,
	query: &str,
	server_matches: &[SeriesKey],
	telemetry: &Recorder,
) -> (Vec<[String; 4]>, Vec<usize>) {
	let suggestions = catalogue.suggestions(query, server_matches, telemetry);
	(suggestions.rows().to_vec(), suggestions.matches().to_vec())
}

fn suggestion_matches(
	catalogue: &mut Catalogue,
	query: &str,
	server_matches: &[SeriesKey],
	telemetry: &Recorder,
) -> Vec<usize> {
	catalogue
		.suggestions(query, server_matches, telemetry)
		.matches()
		.to_vec()
}

#[cfg(test)]
#[path = "series_search_tests.rs"]
mod tests;
