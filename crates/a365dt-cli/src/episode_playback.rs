use std::{
	process::ExitCode,
	sync::{Arc, Mutex},
	time::Instant,
};

use tokio::sync::watch;

use crate::{
	api::{Anime365, Series, Translation},
	continue_watching::{Entry as ContinueWatching, Store as ContinueStore},
	error::Error,
	playback, select,
	telemetry::{self, Recorder, SeriesRecording},
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
	pub first_position: playback::Position,
	pub continuation: Continuation,
	pub telemetry: &'a Recorder,
	pub series_recording: SeriesRecording,
	pub active_playback: Arc<Mutex<Option<watch::Sender<bool>>>>,
	pub continue_watching: &'a ContinueStore,
}

pub(crate) async fn run(request: Request<'_>) -> Result<ExitCode, Error> {
	let Request {
		api,
		series,
		translations,
		track,
		first,
		first_position,
		continuation,
		telemetry,
		series_recording,
		active_playback,
		continue_watching,
	} = request;
	let (cancel, cancellation) = watch::channel(false);
	*active_playback.lock().unwrap() = Some(cancel);
	let result = async {
		let mut release = first;
		let mut position = first_position;
		loop {
			let continue_entry = ContinueWatching::new(series, &release, track)
				.with_position(position);
			continue_watching.save(&continue_entry).await?;
			let title =
				format!("{} — {}", series.title, release.episode.episode_full);
			let started = Instant::now();
			let report = match playback::play(
				api.clone(),
				&release,
				&title,
				position,
				cancellation.clone(),
			)
			.await
			{
				Ok(report) => {
					telemetry.record_playback(
						series,
						started.elapsed(),
						telemetry_outcome(report.outcome),
						series_recording,
					);
					report
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
			if report.outcome == playback::Outcome::NaturalEnd {
				match next_available_episode(&series.episodes, &release.episode)
				{
					Some(episode) => {
						continue_watching
							.save(&continue_entry.with_episode(episode))
							.await?;
					}
					None => continue_watching.clear().await?,
				}
			} else {
				continue_watching
					.save(&continue_entry.with_position(report.position))
					.await?;
			}
			match continuation_action(report.outcome, continuation) {
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
				return Ok(ExitCode::SUCCESS);
			};
			let Some(translation) =
				select::translation_for_track(translations, episode, track)
			else {
				return Ok(ExitCode::SUCCESS);
			};
			let embed = api.embed(translation.id).await?;
			let Some(media_url) = media_url_at_height(&embed, release.height)
			else {
				return Ok(ExitCode::SUCCESS);
			};
			release = select::PlannedRelease {
				episode: episode.clone(),
				translation,
				height: release.height,
				media_url,
				subtitle_url: embed.subtitles_url,
			};
			position = playback::Position::START;
		}
	}
	.await;
	*active_playback.lock().unwrap() = None;
	result
}

fn next_available_episode<'a>(
	episodes: &'a [crate::api::Episode],
	current: &crate::api::Episode,
) -> Option<&'a crate::api::Episode> {
	episodes
		.iter()
		.position(|episode| episode.id == current.id)
		.and_then(|position| episodes.get(position + 1))
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
