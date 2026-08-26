use std::{
	process::ExitCode,
	sync::{Arc, Mutex},
	time::Instant,
};

use tokio::sync::watch;

use crate::{
	api::{Anime365, Series, Translation},
	error::Error,
	playback, select,
	telemetry::{self, Recorder, SeriesRecording},
	ui,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Continuation {
	Disabled,
	Enabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContinuationAction {
	FindNextEpisode,
	Stop(ContinuationStop),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContinuationStop {
	Disabled,
	Interrupted,
	PlayerStoppedOrClosed,
}

pub(crate) struct Request<'a> {
	pub api: Anime365,
	pub series: &'a Series,
	pub translations: &'a [Translation],
	pub track: &'a select::TrackKey,
	pub first: select::PlannedRelease,
	pub continuation: Continuation,
	pub telemetry: &'a Recorder,
	pub series_recording: SeriesRecording,
	pub active_playback: Arc<Mutex<Option<watch::Sender<bool>>>>,
}

pub(crate) async fn run(request: Request<'_>) -> Result<ExitCode, Error> {
	let Request {
		api,
		series,
		translations,
		track,
		first,
		continuation,
		telemetry,
		series_recording,
		active_playback,
	} = request;
	let (cancel, cancellation) = watch::channel(false);
	*active_playback.lock().unwrap() = Some(cancel);
	let result = async {
		let mut release = first;
		loop {
			ui::note(format!(
				"Playing {} — {} in {}p",
				series.title, release.episode.episode_full, release.height
			));
			let title = format!(
				"{} — {}",
				series.title, release.episode.episode_full
			);
			let started = Instant::now();
			let outcome = match playback::play(
				api.clone(),
				&release,
				&title,
				cancellation.clone(),
			)
			.await
			{
				Ok(outcome) => {
					telemetry.record_playback(
						series,
						started.elapsed(),
						telemetry_outcome(outcome),
						series_recording,
					);
					outcome
				}
				Err(error) => {
					telemetry.record_playback(
						series,
						started.elapsed(),
						telemetry::PlaybackOutcome::Failure,
						series_recording,
					);
					return Err(error);
				}
			};
			match continuation_action(outcome, continuation) {
				ContinuationAction::Stop(ContinuationStop::Interrupted) => {
					return Ok(ExitCode::from(130));
				}
				ContinuationAction::Stop(
					ContinuationStop::Disabled
					| ContinuationStop::PlayerStoppedOrClosed,
				) => return Ok(ExitCode::SUCCESS),
				ContinuationAction::FindNextEpisode => {}
			}
			let Some(episode) = playback::next_whole_episode(
				&series.episodes,
				&release.episode,
			) else {
				ui::note("No next whole-number Episode is available.");
				return Ok(ExitCode::SUCCESS);
			};
			let Some(translation) =
				select::translation_for_track(translations, episode, track)
			else {
				ui::note(
					"Automatic continuation stopped: the selected Translation track does not cover the next Episode.",
				);
				return Ok(ExitCode::SUCCESS);
			};
			let embed = api.embed(translation.id).await?;
			let Some(media_url) = media_url_at_height(&embed, release.height) else {
				ui::note(
					"Automatic continuation stopped: the chosen resolution is unavailable for the next Episode.",
				);
				return Ok(ExitCode::SUCCESS);
			};
			release = select::PlannedRelease {
				episode: episode.clone(),
				translation,
				height: release.height,
				media_url,
				subtitle_url: embed.subtitles_url,
			};
		}
	}
	.await;
	*active_playback.lock().unwrap() = None;
	result
}

fn media_url_at_height(
	embed: &crate::api::Embed,
	height: u16,
) -> Option<String> {
	embed
		.download
		.iter()
		.find(|option| option.height == height)
		.and_then(|option| option.url.clone())
}

const fn continuation_action(
	outcome: playback::Outcome,
	continuation: Continuation,
) -> ContinuationAction {
	match (outcome, continuation) {
		(playback::Outcome::Interrupted, _) => {
			ContinuationAction::Stop(ContinuationStop::Interrupted)
		}
		(playback::Outcome::Stopped, _) => {
			ContinuationAction::Stop(ContinuationStop::PlayerStoppedOrClosed)
		}
		(playback::Outcome::NaturalEnd, Continuation::Disabled) => {
			ContinuationAction::Stop(ContinuationStop::Disabled)
		}
		(playback::Outcome::NaturalEnd, Continuation::Enabled) => {
			ContinuationAction::FindNextEpisode
		}
	}
}

fn telemetry_outcome(outcome: playback::Outcome) -> telemetry::PlaybackOutcome {
	match outcome {
		playback::Outcome::Interrupted => {
			telemetry::PlaybackOutcome::Interrupted
		}
		playback::Outcome::NaturalEnd => telemetry::PlaybackOutcome::NaturalEnd,
		playback::Outcome::Stopped => telemetry::PlaybackOutcome::Stopped,
	}
}

#[cfg(test)]
#[path = "episode_playback_tests.rs"]
mod tests;
