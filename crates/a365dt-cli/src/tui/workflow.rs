use std::{
	process::ExitCode,
	sync::{Arc, Mutex},
};

use tokio::sync::watch;

use super::state::{App, ResumeSelection, WorkflowRequest};
use crate::{
	api::{Anime365, Embed, Translation},
	cache,
	content::ContentSource,
	episode_playback,
	error::Error,
	interactive,
	preferences::Preferences,
	series_search::Selection,
	telemetry::Recorder,
};

pub(super) enum Loaded {
	Series(Result<Selection, Error>),
	Resume(Result<Box<ResumeSelection>, Error>),
	Translations(Result<Vec<Translation>, Error>),
	Media(Result<Embed, Error>),
}

pub(super) struct LoadContext<'a> {
	pub apis: &'a [Anime365],
	pub store: &'a cache::Store,
}

pub(super) async fn load(
	request: WorkflowRequest,
	context: LoadContext<'_>,
) -> Loaded {
	match request {
		WorkflowRequest::Series(launch) => Loaded::Series(
			interactive::selection(launch, context.apis, context.store).await,
		),
		WorkflowRequest::Resume(entry) => {
			Loaded::Resume(load_resume(entry, context.apis).await.map(Box::new))
		}
		WorkflowRequest::Translations { source, series_id } => {
			let result = match api_for_source(context.apis, source) {
				Ok(api) => api.translations(series_id).await,
				Err(error) => Err(error),
			};
			Loaded::Translations(result)
		}
		WorkflowRequest::Media {
			source,
			translation_id,
		} => {
			let result = match api_for_source(context.apis, source) {
				Ok(api) => api.embed(translation_id).await,
				Err(error) => Err(error),
			};
			Loaded::Media(result)
		}
		WorkflowRequest::Playback(_) => {
			unreachable!("Playback owns the foreground Player handoff")
		}
	}
}

pub(super) fn apply(
	app: &mut App,
	loaded: Loaded,
	preferences: &Preferences,
	telemetry: &Recorder,
) -> Option<u16> {
	match loaded {
		Loaded::Series(Ok(selection)) => {
			let recording =
				crate::series_recording(&selection.series, preferences);
			telemetry.record_series(
				&selection.series,
				selection.catalogue,
				recording,
			);
			app.set_selection(selection);
		}
		Loaded::Translations(Ok(translations)) => {
			app.set_translations(translations);
		}
		Loaded::Media(Ok(embed))
			if crate::select::available_heights(&embed).is_empty() =>
		{
			app.fail_workflow(
				"Anime365 returned no playable resolutions.".into(),
			);
		}
		Loaded::Media(Ok(embed)) => app.set_embed(embed),
		Loaded::Resume(Ok(resume)) => {
			let height = resume.height;
			app.set_resume(*resume);
			return Some(height);
		}
		Loaded::Series(Err(error))
		| Loaded::Resume(Err(error))
		| Loaded::Translations(Err(error))
		| Loaded::Media(Err(error)) => {
			app.fail_workflow(error.message().to_owned());
		}
	}
	None
}

async fn load_resume(
	entry: crate::continue_watching::Entry,
	apis: &[Anime365],
) -> Result<ResumeSelection, Error> {
	let api = api_for_source(apis, entry.series.source)?;
	let series = api.series(entry.series.id).await?.ok_or_else(|| {
		Error::new("That Continue Watching Series is unavailable.")
	})?;
	let episode = series
		.episodes
		.iter()
		.find(|episode| episode.id == entry.episode_id)
		.cloned()
		.ok_or_else(|| {
			Error::new("That Continue Watching Episode is no longer available.")
		})?;
	let translations = api.translations(series.id).await?;
	let translation = crate::select::translation_for_track(
		&translations,
		&episode,
		&entry.track,
	)
	.ok_or_else(|| {
		Error::new(
			"The remembered Continue Watching Translation is no longer available.",
		)
	})?;
	let embed = api.embed(translation.id).await?;
	if !crate::select::available_heights(&embed).contains(&entry.height) {
		return Err(Error::new(
			"The remembered Continue Watching resolution is no longer available.",
		));
	}
	Ok(ResumeSelection {
		series,
		episode,
		translations,
		track: entry.track,
		translation,
		embed,
		height: entry.height,
		position: entry.position,
	})
}

pub(super) struct PlaybackContext<'a> {
	pub apis: &'a [Anime365],
	pub preferences: &'a Preferences,
	pub telemetry: &'a Recorder,
	pub session_cancel: &'a watch::Sender<bool>,
	pub active_playback: &'a Arc<Mutex<Option<watch::Sender<bool>>>>,
	pub continue_watching: &'a crate::continue_watching::Store,
}

pub(super) async fn play(
	app: &mut App,
	height: u16,
	context: PlaybackContext<'_>,
) -> Result<Option<ExitCode>, Error> {
	let Some(selection) = app.playback(height) else {
		app.fail_workflow("That resolution is no longer available.".into());
		return Ok(None);
	};
	let api = match api_for_source(context.apis, selection.series.source) {
		Ok(api) => api.clone(),
		Err(error) => {
			app.fail_workflow(error.message().to_owned());
			return Ok(None);
		}
	};
	let recording =
		crate::series_recording(&selection.series, context.preferences);
	let result = episode_playback::run(episode_playback::Request {
		api,
		series: &selection.series,
		translations: &selection.translations,
		track: &selection.track,
		first: selection.release,
		first_position: selection.position,
		continuation: if context.preferences.auto_play_next_episode {
			episode_playback::Continuation::Enabled
		} else {
			episode_playback::Continuation::Disabled
		},
		telemetry: context.telemetry,
		series_recording: recording,
		active_playback: Arc::clone(context.active_playback),
		continue_watching: context.continue_watching,
	})
	.await;
	*context.active_playback.lock().unwrap() =
		Some(context.session_cancel.clone());
	match result {
		Ok(exit) if exit == ExitCode::SUCCESS => Ok(None),
		Ok(exit) => Ok(Some(exit)),
		Err(error) => {
			app.fail_workflow(error.message().to_owned());
			Ok(None)
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
