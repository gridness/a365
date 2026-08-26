use std::collections::HashSet;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::{
	anilist,
	api::{Anime365, Series},
	cache,
	community::{CommunityClient, MomentCategory, MomentPage},
	content::ContentSource,
	error::Error,
	preferences::{AdultContent, Preferences},
};

mod state;
mod terminal;
mod view;

use state::{
	AniListView, App, Data, ProfileView, SeriesView, SourceWarning, Surface,
	Update,
};
pub(crate) use state::{Destination, Launch};

const MAX_CATALOGUE_SIZE: usize = 100_000;

#[cfg(test)]
#[path = "../tui_tests.rs"]
mod tests;

enum Loaded {
	Series(Result<SeriesView, Error>),
	Timetable(Result<Vec<anilist::ScheduleEntry>, Error>),
	Moments(Result<MomentPage, Error>),
	AniList(Result<AniListView, Error>),
	Profile(Result<ProfileView, Error>),
}

struct Loaders(Vec<JoinHandle<()>>);

impl Loaders {
	fn push(&mut self, loader: JoinHandle<()>) {
		self.0.push(loader);
	}
}

impl Drop for Loaders {
	fn drop(&mut self) {
		for loader in &self.0 {
			loader.abort();
		}
	}
}

pub(crate) async fn run(
	apis: &[Anime365],
	store: &cache::Store,
	preferences: &Preferences,
	destination: Destination,
	query: String,
	cancelled: watch::Receiver<bool>,
) -> Result<Option<Launch>, Error> {
	let data = Data {
		series: Surface::Loading,
		timetable: Surface::Loading,
		moments: Surface::Loading,
		anilist: Surface::Loading,
		profile: Surface::Loading,
	};
	let mut app = App::new(destination, query, data);
	let (updates, mut loaded) = mpsc::unbounded_channel();
	let adult = preferences.adult_content();
	let anime365 = apis
		.iter()
		.find(|api| api.source() == ContentSource::Anime365)
		.cloned();
	let mut loaders = spawn_loaders(apis, store, adult, updates.clone());
	let mut terminal = terminal::Session::enter()?;
	loop {
		if *cancelled.borrow() {
			return Ok(None);
		}
		while let Ok(update) = loaded.try_recv() {
			apply_loaded(&mut app, update);
		}
		let hit_map = terminal.draw(&mut app)?;
		let Some(event) = terminal.event(&hit_map)? else {
			continue;
		};
		match app.update(event) {
			Update::Continue => {}
			Update::Launch(launch) => return Ok(Some(launch)),
			Update::LoadMoments { page, category } => {
				loaders.push(spawn_moment_loader(
					anime365.clone(),
					adult,
					page,
					category,
					updates.clone(),
				));
			}
			Update::Quit => return Ok(None),
		}
	}
}

fn spawn_loaders(
	apis: &[Anime365],
	store: &cache::Store,
	adult: AdultContent,
	updates: mpsc::UnboundedSender<Loaded>,
) -> Loaders {
	let anime365 = apis
		.iter()
		.find(|api| api.source() == ContentSource::Anime365)
		.cloned();
	let series_apis = apis.to_vec();
	let series_store = store.clone();
	let sender = updates.clone();
	let mut loaders = vec![tokio::spawn(async move {
		let result = load_series(series_apis, series_store).await;
		let _ = sender.send(Loaded::Series(result));
	})];
	let sender = updates.clone();
	loaders.push(tokio::spawn(async move {
		let _ =
			sender.send(Loaded::Timetable(anilist::current_week(adult).await));
	}));
	let sender = updates.clone();
	let moments_api = anime365.clone();
	loaders.push(tokio::spawn(async move {
		let result = load_moments(moments_api, adult, 1, None).await;
		let _ = sender.send(Loaded::Moments(result));
	}));
	let sender = updates.clone();
	loaders.push(tokio::spawn(async move {
		let result = load_anilist(adult).await;
		let _ = sender.send(Loaded::AniList(result));
	}));
	loaders.push(tokio::spawn(async move {
		let result = load_profile(anime365, adult).await;
		let _ = updates.send(Loaded::Profile(result));
	}));
	Loaders(loaders)
}

fn apply_loaded(app: &mut App, loaded: Loaded) {
	match loaded {
		Loaded::Series(result) => app.set_series(series_surface(result)),
		Loaded::Timetable(result) => app.set_timetable(surface(result)),
		Loaded::Moments(result) => app.set_moments(moment_surface(result)),
		Loaded::AniList(result) => app.set_anilist(scalar_surface(result)),
		Loaded::Profile(result) => app.set_profile(scalar_surface(result)),
	}
}

fn surface<T>(result: Result<Vec<T>, Error>) -> Surface<Vec<T>> {
	match result {
		Ok(values) if values.is_empty() => Surface::Empty,
		Ok(values) => Surface::Ready(values),
		Err(error) => Surface::Error(error.message().to_owned()),
	}
}

fn series_surface(result: Result<SeriesView, Error>) -> Surface<SeriesView> {
	match result {
		Ok(view) if view.series.is_empty() && view.warnings.is_empty() => {
			Surface::Empty
		}
		Ok(view) => Surface::Ready(view),
		Err(error) => Surface::Error(error.message().to_owned()),
	}
}

fn scalar_surface<T>(result: Result<T, Error>) -> Surface<T> {
	match result {
		Ok(value) => Surface::Ready(value),
		Err(error) => Surface::Error(error.message().to_owned()),
	}
}

fn moment_surface(result: Result<MomentPage, Error>) -> Surface<MomentPage> {
	match result {
		Ok(page) if page.moments.is_empty() && page.categories.is_empty() => {
			Surface::Empty
		}
		Ok(page) => Surface::Ready(page),
		Err(error) => Surface::Error(error.message().to_owned()),
	}
}

async fn load_series(
	apis: Vec<Anime365>,
	store: cache::Store,
) -> Result<SeriesView, Error> {
	if apis.is_empty() {
		return Err(anime365_unavailable("Search"));
	}
	let loaded = store.load_catalogue().await?;
	let (mut catalogue, writer) =
		loaded.into_session(&store, Default::default());
	let sources = apis.iter().map(Anime365::source).collect::<HashSet<_>>();
	catalogue.retain_sources(&sources);
	let mut failures = Vec::new();
	let mut successful_refreshes = 0;
	for api in apis {
		let source = api.source();
		if catalogue.is_fresh_for(&HashSet::from([source])) {
			continue;
		}
		match load_source(api).await {
			Ok(series) => {
				successful_refreshes += 1;
				writer.commit_refresh(source, series.clone());
				catalogue.remove_source(source);
				catalogue.upsert(series);
			}
			Err(error) => {
				if source == ContentSource::H365 {
					catalogue.remove_source(source);
				}
				failures.push((source, error));
			}
		}
	}
	let writer_failure = writer.finish().await.err();
	if catalogue.is_empty() && successful_refreshes == 0 {
		if !failures.is_empty() {
			return Err(failures.remove(0).1);
		}
		if let Some(error) = writer_failure {
			return Err(error);
		}
	}
	let warnings = failures
		.into_iter()
		.map(|(source, error)| SourceWarning {
			source,
			message: error.message().to_owned(),
		})
		.collect::<Vec<_>>();
	Ok(SeriesView {
		series: catalogue.all_series().to_vec(),
		warnings,
	})
}

async fn load_source(api: Anime365) -> Result<Vec<Series>, Error> {
	let mut series = Vec::new();
	loop {
		let page = api.series_page(series.len()).await?;
		let complete = page.len() < crate::api::SERIES_PAGE_SIZE;
		series.extend(page);
		if complete {
			return Ok(series);
		}
		if series.len() >= MAX_CATALOGUE_SIZE {
			return Err(Error::new(format!(
				"The {} catalogue exceeded its safe size limit.",
				api.source()
			)));
		}
	}
}

async fn load_anilist(adult: AdultContent) -> Result<AniListView, Error> {
	let client = anilist::Client::connected()?.ok_or_else(|| {
		Error::new("AniList is not connected. Run `a365 anilist login`.")
	})?;
	let viewer = client.viewer().await?;
	let library = client.library(viewer.id, adult).await?;
	Ok(AniListView { viewer, library })
}

async fn load_profile(
	anime365: Option<Anime365>,
	adult: AdultContent,
) -> Result<ProfileView, Error> {
	let anime365 = anime365.ok_or_else(|| anime365_unavailable("Profile"))?;
	let community = CommunityClient::new()?;
	let documented = anime365.profile().await?;
	let enrichment = match documented.id {
		Some(id) => community
			.profile(id, &anime365, adult)
			.await
			.map_err(|error| error.to_string()),
		None => Err("Anime365 did not return a profile ID.".to_owned()),
	};
	Ok(ProfileView {
		documented,
		enrichment,
	})
}

async fn load_moments(
	anime365: Option<Anime365>,
	adult: AdultContent,
	page: u32,
	category: Option<MomentCategory>,
) -> Result<MomentPage, Error> {
	let anime365 = anime365.ok_or_else(|| anime365_unavailable("Moments"))?;
	CommunityClient::new()?
		.moments(page, category.as_ref(), &anime365, adult)
		.await
}

fn spawn_moment_loader(
	anime365: Option<Anime365>,
	adult: AdultContent,
	page: u32,
	category: Option<MomentCategory>,
	updates: mpsc::UnboundedSender<Loaded>,
) -> JoinHandle<()> {
	tokio::spawn(async move {
		let result = load_moments(anime365, adult, page, category).await;
		let _ = updates.send(Loaded::Moments(result));
	})
}

fn anime365_unavailable(surface: &str) -> Error {
	Error::new(format!(
		"{surface} needs Anime365 access. Start a365 without an AniList or Timetable command to connect."
	))
}
