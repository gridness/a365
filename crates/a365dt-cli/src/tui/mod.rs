use std::{
	collections::HashSet,
	process::ExitCode,
	sync::{Arc, Mutex},
};

use tokio::sync::{mpsc, watch};
use tokio::{
	task::{JoinHandle, JoinSet},
	time::sleep,
};

use crate::{
	anilist,
	api::Anime365,
	cache,
	community::{CommunityClient, MomentCategory, MomentPage},
	content::ContentSource,
	continue_watching,
	error::Error,
	interactive,
	preferences::{AdultContent, Overrides, Preferences},
	series_search,
	telemetry::Recorder,
};

mod config;
mod state;
mod terminal;
mod view;
mod workflow;

use state::{
	AniListView, App, Data, HomeView, ProfileView, SeriesView, SourceWarning,
	Surface, Update, WorkflowRequest,
};
pub(crate) use state::{Destination, Launch};

#[cfg(test)]
#[path = "../tui_tests.rs"]
mod tests;

enum Loaded {
	ContinueWatching(Result<Option<continue_watching::Entry>, Error>),
	TrendingSeries(Result<Vec<anilist::TrendingSeries>, Error>),
	TrendingMoments(Result<MomentPage, Error>),
	Series(Result<SeriesView, Error>),
	Timetable(Result<Vec<anilist::ScheduleEntry>, Error>),
	Moments(Result<MomentPage, Error>),
	AniList {
		revision: u64,
		result: Result<Option<AniListView>, Error>,
	},
	Profile(Result<ProfileView, Error>),
	Search {
		query: String,
		result: crate::api::Result<series_search::RemoteResults>,
	},
	Workflow {
		revision: u64,
		result: workflow::Loaded,
	},
	Config {
		revision: u64,
		result: Result<config::Applied, Error>,
	},
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

pub(crate) struct Request<'a> {
	pub apis: &'a [Anime365],
	pub store: &'a cache::Store,
	pub preferences: &'a Preferences,
	pub configured_preferences: &'a Preferences,
	pub preference_store: &'a crate::preferences::Store,
	pub overrides: Overrides,
	pub telemetry: &'a Recorder,
	pub destination: Destination,
	pub query: String,
	pub cancelled: watch::Receiver<bool>,
	pub session_cancel: watch::Sender<bool>,
	pub active_playback: Arc<Mutex<Option<watch::Sender<bool>>>>,
	pub continue_watching: &'a continue_watching::Store,
	pub tip: Option<String>,
	pub upgrade: Option<String>,
	pub debug: bool,
}

pub(crate) async fn run(request: Request<'_>) -> Result<ExitCode, Error> {
	let Request {
		apis: initial_apis,
		store,
		preferences: initial_preferences,
		configured_preferences: initial_configured_preferences,
		preference_store,
		overrides,
		telemetry,
		destination,
		query,
		cancelled,
		session_cancel,
		active_playback,
		continue_watching,
		tip,
		upgrade,
		debug,
	} = request;
	let data = Data {
		home: HomeView::loading(tip),
		upgrade,
		debug,
		series: Surface::Loading,
		timetable: Surface::Loading,
		moments: Surface::Loading,
		anilist: Surface::Loading,
		profile: Surface::Loading,
		config: state::ConfigView::new(initial_configured_preferences.clone()),
	};
	let mut apis = initial_apis.to_vec();
	let mut preferences = initial_preferences.clone();
	let mut configured_preferences = initial_configured_preferences.clone();
	let mut app = App::new(destination, query, data, telemetry.clone());
	let (updates, mut loaded) = mpsc::unbounded_channel();
	let mut loaders = spawn_loaders(
		&apis,
		store,
		preferences.adult_content(),
		telemetry,
		continue_watching,
		app.anilist_revision(),
		updates.clone(),
	);
	let mut search_loader = spawn_search_loader(
		&apis,
		app.query.clone(),
		telemetry,
		updates.clone(),
	);
	let mut terminal = terminal::Session::enter()?;
	loop {
		if *cancelled.borrow() {
			return Ok(ExitCode::from(130));
		}
		while let Ok(update) = loaded.try_recv() {
			let height = match update {
				Loaded::Config { revision, result } => {
					if revision == app.config_revision() {
						match result {
							Ok(applied) => {
								let content_changed = applied.content_changed;
								preferences = applied.effective;
								apis = applied.apis;
								configured_preferences =
									applied.configured.clone();
								app.finish_config(
									applied.configured,
									applied.message,
								);
								if content_changed {
									app.prepare_content_reload();
									loaders = spawn_loaders(
										&apis,
										store,
										preferences.adult_content(),
										telemetry,
										continue_watching,
										app.anilist_revision(),
										updates.clone(),
									);
									if let Some(loader) = search_loader.take() {
										loader.abort();
									}
									search_loader = spawn_search_loader(
										&apis,
										app.query.clone(),
										telemetry,
										updates.clone(),
									);
								}
							}
							Err(error) => {
								app.fail_config(error.message().to_owned());
							}
						}
					}
					None
				}
				update => {
					apply_loaded(&mut app, update, &preferences, telemetry)
				}
			};
			if let Some(height) = height {
				let _ = terminal.draw(&mut app)?;
				let exit = workflow::play(
					&mut app,
					height,
					workflow::PlaybackContext {
						apis: &apis,
						preferences: &preferences,
						telemetry,
						session_cancel: &session_cancel,
						active_playback: &active_playback,
						continue_watching,
					},
				)
				.await?;
				app.finish_playback();
				refresh_continue_watching(&mut app, continue_watching).await;
				if let Some(exit) = exit {
					return Ok(exit);
				}
			}
		}
		let hit_map = terminal.draw(&mut app)?;
		let Some(event) = terminal.event(&hit_map)? else {
			continue;
		};
		match app.update(event) {
			Update::Continue => {}
			Update::Search(query) => {
				if let Some(loader) = search_loader.take() {
					loader.abort();
				}
				search_loader = spawn_search_loader(
					&apis,
					query,
					telemetry,
					updates.clone(),
				);
			}
			Update::Workflow { revision, request } => match request {
				WorkflowRequest::Playback(height) => {
					let _ = terminal.draw(&mut app)?;
					let exit = workflow::play(
						&mut app,
						height,
						workflow::PlaybackContext {
							apis: &apis,
							preferences: &preferences,
							telemetry,
							session_cancel: &session_cancel,
							active_playback: &active_playback,
							continue_watching,
						},
					)
					.await?;
					app.finish_playback();
					refresh_continue_watching(&mut app, continue_watching)
						.await;
					if let Some(exit) = exit {
						return Ok(exit);
					}
				}
				request => loaders.push(spawn_workflow_loader(
					revision,
					request,
					&apis,
					store,
					updates.clone(),
				)),
			},
			Update::PlayMoment { id, title } => {
				let result = interactive::play_moment(
					id,
					&title,
					Arc::clone(&active_playback),
				)
				.await;
				*active_playback.lock().unwrap() = Some(session_cancel.clone());
				match result {
					Ok(exit) if exit == ExitCode::SUCCESS => {}
					Ok(exit) => return Ok(exit),
					Err(error) => return Err(error),
				}
			}
			Update::ConnectAniList { revision } => {
				loaders.push(spawn_anilist_connection(
					revision,
					preferences.adult_content(),
					updates.clone(),
				));
			}
			Update::LoadMoments { page, category } => {
				let anime365 = apis
					.iter()
					.find(|api| api.source() == ContentSource::Anime365)
					.cloned();
				loaders.push(spawn_moment_loader(
					anime365,
					preferences.adult_content(),
					page,
					category,
					updates.clone(),
				));
			}
			Update::SaveConfig { revision, change } => {
				let preference_store = preference_store.clone();
				let overrides = overrides.clone();
				let configured = configured_preferences.clone();
				let config_apis = apis.clone();
				let sender = updates.clone();
				loaders.push(tokio::spawn(async move {
					let result = config::apply(
						preference_store,
						overrides,
						configured,
						config_apis,
						change,
					)
					.await;
					let _ = sender.send(Loaded::Config { revision, result });
				}));
			}
			Update::Quit => return Ok(ExitCode::SUCCESS),
		}
	}
}

fn spawn_loaders(
	apis: &[Anime365],
	store: &cache::Store,
	adult: AdultContent,
	telemetry: &Recorder,
	continue_watching: &continue_watching::Store,
	anilist_revision: u64,
	updates: mpsc::UnboundedSender<Loaded>,
) -> Loaders {
	let anime365 = apis
		.iter()
		.find(|api| api.source() == ContentSource::Anime365)
		.cloned();
	let continue_watching = continue_watching.clone();
	let sender = updates.clone();
	let mut loaders = vec![tokio::spawn(async move {
		let _ = sender
			.send(Loaded::ContinueWatching(continue_watching.load().await));
	})];
	let sender = updates.clone();
	loaders.push(tokio::spawn(async move {
		let _ = sender
			.send(Loaded::TrendingSeries(anilist::trending_series().await));
	}));
	let trending_api = anime365.clone();
	let sender = updates.clone();
	loaders.push(tokio::spawn(async move {
		let result = load_trending_moments(trending_api).await;
		let _ = sender.send(Loaded::TrendingMoments(result));
	}));
	let series_apis = apis.to_vec();
	let series_store = store.clone();
	let series_telemetry = telemetry.clone();
	let sender = updates.clone();
	loaders.push(tokio::spawn(async move {
		load_series(series_apis, series_store, series_telemetry, sender).await;
	}));
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
		let _ = sender.send(Loaded::AniList {
			revision: anilist_revision,
			result,
		});
	}));
	loaders.push(tokio::spawn(async move {
		let result = load_profile(anime365, adult).await;
		let _ = updates.send(Loaded::Profile(result));
	}));
	Loaders(loaders)
}

fn apply_loaded(
	app: &mut App,
	loaded: Loaded,
	preferences: &Preferences,
	telemetry: &Recorder,
) -> Option<u16> {
	match loaded {
		Loaded::ContinueWatching(result) => {
			app.set_continue_watching(optional_surface(result));
		}
		Loaded::TrendingSeries(result) => {
			app.set_trending_series(surface(result));
		}
		Loaded::TrendingMoments(result) => {
			app.set_trending_moments(moment_surface(result));
		}
		Loaded::Series(result) => app.set_series(series_surface(result)),
		Loaded::Timetable(result) => app.set_timetable(surface(result)),
		Loaded::Moments(result) => app.set_moments(moment_surface(result)),
		Loaded::AniList { revision, result }
			if revision == app.anilist_revision() =>
		{
			app.set_anilist(optional_surface(result));
		}
		Loaded::AniList { .. } => {}
		Loaded::Profile(result) => app.set_profile(scalar_surface(result)),
		Loaded::Search { query, result } => match result {
			Ok(mut results) => {
				results.exact.append(&mut results.fallback);
				let warning = (!results.warnings.is_empty()).then(|| {
					results
						.warnings
						.iter()
						.map(|error| error.message())
						.collect::<Vec<_>>()
						.join(" · ")
				});
				app.set_remote_search(query, results.exact, warning);
			}
			Err(error) => app.set_remote_search(
				query,
				Vec::new(),
				Some(error.message().to_owned()),
			),
		},
		Loaded::Workflow { revision, result }
			if revision == app.workflow_revision() =>
		{
			return workflow::apply(app, result, preferences, telemetry);
		}
		Loaded::Workflow { .. } => {}
		Loaded::Config { .. } => {}
	}
	None
}

async fn refresh_continue_watching(
	app: &mut App,
	store: &continue_watching::Store,
) {
	app.set_continue_watching(optional_surface(store.load().await));
}

fn spawn_workflow_loader(
	revision: u64,
	request: WorkflowRequest,
	apis: &[Anime365],
	store: &cache::Store,
	updates: mpsc::UnboundedSender<Loaded>,
) -> JoinHandle<()> {
	let apis = apis.to_vec();
	let store = store.clone();
	tokio::spawn(async move {
		let result = workflow::load(
			request,
			workflow::LoadContext {
				apis: &apis,
				store: &store,
			},
		)
		.await;
		let _ = updates.send(Loaded::Workflow { revision, result });
	})
}

fn spawn_anilist_connection(
	revision: u64,
	adult: AdultContent,
	updates: mpsc::UnboundedSender<Loaded>,
) -> JoinHandle<()> {
	tokio::spawn(async move {
		let result = connect_anilist(adult).await.map(Some);
		let _ = updates.send(Loaded::AniList { revision, result });
	})
}

fn spawn_search_loader(
	apis: &[Anime365],
	query: String,
	telemetry: &Recorder,
	updates: mpsc::UnboundedSender<Loaded>,
) -> Option<JoinHandle<()>> {
	if query.trim().is_empty() {
		return None;
	}
	let apis = apis.to_vec();
	let telemetry = telemetry.clone();
	Some(tokio::spawn(async move {
		sleep(series_search::SEARCH_DEBOUNCE).await;
		let result =
			series_search::remote_search(&apis, &query, &telemetry).await;
		let _ = updates.send(Loaded::Search { query, result });
	}))
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

fn optional_surface<T>(result: Result<Option<T>, Error>) -> Surface<T> {
	match result {
		Ok(Some(value)) => Surface::Ready(value),
		Ok(None) => Surface::Empty,
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
	telemetry: Recorder,
	updates: mpsc::UnboundedSender<Loaded>,
) {
	if apis.is_empty() {
		let _ =
			updates.send(Loaded::Series(Err(anime365_unavailable("Search"))));
		return;
	}
	let loaded = match store.load_catalogue().await {
		Ok(loaded) => loaded,
		Err(error) => {
			let _ = updates.send(Loaded::Series(Err(error)));
			return;
		}
	};
	let (mut catalogue, writer) =
		loaded.into_session(&store, telemetry.clone());
	let sources = apis.iter().map(Anime365::source).collect::<HashSet<_>>();
	catalogue.retain_sources(&sources);
	let stale = apis
		.into_iter()
		.filter(|api| !catalogue.is_fresh_for(&HashSet::from([api.source()])))
		.collect::<Vec<_>>();
	let mut warnings = Vec::new();
	catalogue.prepare_search(&telemetry);
	let _ = updates.send(Loaded::Series(Ok(SeriesView::from_catalogue(
		catalogue.clone(),
		warnings.clone(),
	))));

	let mut refreshes = JoinSet::new();
	for api in stale {
		let source = api.source();
		refreshes.spawn(async move {
			(source, series_search::refresh_source(api, |_, _| {}).await)
		});
	}
	while let Some(joined) = refreshes.join_next().await {
		let Ok((source, result)) = joined else {
			continue;
		};
		warnings.retain(|warning: &SourceWarning| warning.source != source);
		match result {
			Ok(series) => {
				writer.commit_refresh(source, series.clone());
				catalogue.remove_source(source);
				catalogue.upsert(series);
			}
			Err(error) => {
				if source == ContentSource::H365 {
					catalogue.remove_source(source);
				}
				warnings.push(SourceWarning {
					source,
					message: error.message().to_owned(),
				});
			}
		}
		catalogue.prepare_search(&telemetry);
		let _ = updates.send(Loaded::Series(Ok(SeriesView::from_catalogue(
			catalogue.clone(),
			warnings.clone(),
		))));
	}
	if let Err(error) = writer.finish().await
		&& catalogue.is_empty()
	{
		let _ = updates.send(Loaded::Series(Err(error)));
	}
}

async fn load_anilist(
	adult: AdultContent,
) -> Result<Option<AniListView>, Error> {
	let Some(client) = anilist::Client::connected()? else {
		return Ok(None);
	};
	let viewer = client.viewer().await?;
	let library = client.library(viewer.id, adult).await?;
	Ok(Some(AniListView { viewer, library }))
}

async fn connect_anilist(adult: AdultContent) -> Result<AniListView, Error> {
	let (viewer, library) = anilist::connect_library(adult).await?;
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

async fn load_trending_moments(
	anime365: Option<Anime365>,
) -> Result<MomentPage, Error> {
	let anime365 =
		anime365.ok_or_else(|| anime365_unavailable("Trending Moments"))?;
	CommunityClient::new()?.trending_moments(&anime365).await
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
