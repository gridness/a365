use std::{
	collections::HashSet,
	process::ExitCode,
	sync::{Arc, Mutex},
};

use tokio::sync::watch;

use super::{Args, Commands};
use crate::{
	anilist, api::Anime365, cache, community::CommunityClient,
	content::ContentSource, error::Error, playback, series_search,
	telemetry::CatalogueUse, tui,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Anime365Access {
	Required,
	Deferred,
}

pub(crate) fn anime365_access(args: &Args) -> Anime365Access {
	match &args.command {
		Some(Commands::Anilist {
			command: anilist::Command::Login | anilist::Command::List,
		})
		| Some(Commands::Timetable) => Anime365Access::Deferred,
		Some(Commands::Anilist {
			command: anilist::Command::Status | anilist::Command::Logout { .. },
		})
		| Some(Commands::Cache { .. })
		| Some(Commands::Completions { .. })
		| Some(Commands::Config { .. })
		| Some(Commands::Doctor { .. })
		| Some(Commands::Moments)
		| Some(Commands::Profile)
		| Some(Commands::Purge { .. })
		| Some(Commands::Stats { .. })
		| Some(Commands::Stream { .. })
		| Some(Commands::Telemetry { .. })
		| Some(Commands::Update { .. })
		| None => Anime365Access::Required,
	}
}

pub(crate) fn destination(args: &Args, query: &str) -> tui::Destination {
	match &args.command {
		Some(Commands::Anilist {
			command: anilist::Command::Login | anilist::Command::List,
		}) => tui::Destination::AniList,
		Some(Commands::Moments) => tui::Destination::Moments,
		Some(Commands::Profile) => tui::Destination::Profile,
		Some(Commands::Timetable) => tui::Destination::Timetable,
		Some(Commands::Stream { .. }) | None if query.is_empty() => {
			tui::Destination::Home
		}
		Some(Commands::Stream { .. }) | None => tui::Destination::Search,
		Some(Commands::Anilist {
			command: anilist::Command::Status | anilist::Command::Logout { .. },
		})
		| Some(Commands::Cache { .. })
		| Some(Commands::Completions { .. })
		| Some(Commands::Config { .. })
		| Some(Commands::Doctor { .. })
		| Some(Commands::Purge { .. })
		| Some(Commands::Stats { .. })
		| Some(Commands::Telemetry { .. })
		| Some(Commands::Update { .. }) => tui::Destination::Home,
	}
}

pub(crate) async fn selection(
	launch: tui::Launch,
	apis: &[Anime365],
	store: &cache::Store,
) -> Result<series_search::Selection, Error> {
	match launch {
		tui::Launch::Series(key) => {
			let series = api_for_source(apis, key.source)?
				.series(key.id)
				.await?
				.ok_or_else(|| {
					Error::new("That selected series is no longer available.")
				})?;
			Ok(series_search::Selection {
				series,
				catalogue: CatalogueUse::Hit,
			})
		}
		tui::Launch::ExternalSeries {
			my_anime_list_id,
			anilist_id,
			title,
		} => {
			let mut catalogue = store.load_catalogue().await?.into_catalogue();
			let sources =
				apis.iter().map(Anime365::source).collect::<HashSet<_>>();
			catalogue.retain_sources(&sources);
			if let Some(series) = catalogue
				.external_match(my_anime_list_id, anilist_id, &sources)
				.cloned() && let Some(series) =
				api_for_source(apis, series.source)?
					.series(series.id)
					.await?
			{
				return Ok(series_search::Selection {
					series,
					catalogue: CatalogueUse::Hit,
				});
			}
			for api in apis {
				let candidates = api.search(&title).await?;
				let candidate = candidates
					.iter()
					.find(|series| {
						my_anime_list_id.is_some_and(|id| {
							series.my_anime_list_id == Some(id)
						}) || series.anilist_id == Some(anilist_id)
					})
					.or_else(|| {
						candidates.iter().find(|series| {
							series.title.eq_ignore_ascii_case(&title)
						})
					});
				if let Some(candidate) = candidate
					&& let Some(series) = api.series(candidate.id).await?
				{
					return Ok(series_search::Selection {
						series,
						catalogue: CatalogueUse::Miss,
					});
				}
			}
			Err(Error::new(format!(
				"{title} is not currently available from an enabled Anime365 source."
			)))
		}
	}
}

pub(crate) async fn play_moment(
	id: u64,
	title: &str,
	active_playback: Arc<Mutex<Option<watch::Sender<bool>>>>,
) -> Result<ExitCode, Error> {
	let media = CommunityClient::new()?.moment_media(id).await?;
	let (cancel, cancellation) = watch::channel(false);
	*active_playback.lock().unwrap() = Some(cancel);
	let result = playback::play_public(media.url, title, cancellation).await;
	*active_playback.lock().unwrap() = None;
	match result?.outcome {
		playback::Outcome::Interrupted => Ok(ExitCode::from(130)),
		playback::Outcome::NaturalEnd | playback::Outcome::Stopped => {
			Ok(ExitCode::SUCCESS)
		}
	}
}

fn api_for_source(
	apis: &[Anime365],
	source: ContentSource,
) -> Result<&Anime365, Error> {
	apis.iter()
		.find(|api| api.source() == source)
		.ok_or_else(|| Error::new(format!("{source} is not enabled.")))
}
