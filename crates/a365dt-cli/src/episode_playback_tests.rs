use pretty_assertions::assert_eq;

use super::{
	Continuation, ContinuationAction, ContinuationStop, continuation_action,
	media_url_at_height, next_available_episode,
};
use crate::{
	api::{Embed, Episode, MediaOption},
	content::ContentSource,
	playback::Outcome,
};

fn episode(id: u64, label: &str) -> Episode {
	Episode {
		source: ContentSource::Anime365,
		id,
		episode_int: label.into(),
		episode_full: label.into(),
	}
}

#[test]
fn continuation_requires_opt_in_and_a_natural_end_of_file() {
	assert_eq!(
		[
			continuation_action(Outcome::NaturalEnd, Continuation::Enabled),
			continuation_action(Outcome::NaturalEnd, Continuation::Disabled),
			continuation_action(Outcome::Stopped, Continuation::Enabled),
			continuation_action(Outcome::Interrupted, Continuation::Enabled),
		],
		[
			ContinuationAction::FindNextEpisode,
			ContinuationAction::Stop(ContinuationStop::Disabled),
			ContinuationAction::Stop(ContinuationStop::PlayerStoppedOrClosed,),
			ContinuationAction::Stop(ContinuationStop::Interrupted),
		],
	);
}

#[test]
fn continuation_resolution_lookup_stops_when_coverage_is_missing() {
	let embed = Embed {
		download: vec![MediaOption {
			height: 720,
			url: Some("https://example.com/720.mp4".into()),
		}],
		subtitles_url: None,
	};

	assert_eq!(
		(
			media_url_at_height(&embed, 720),
			media_url_at_height(&embed, 1080),
		),
		(Some("https://example.com/720.mp4".into()), None),
	);
}

#[test]
fn continue_watching_advances_to_the_next_available_episode() {
	let episodes = vec![episode(1, "1"), episode(2, "1.5"), episode(3, "2")];

	assert_eq!(
		[
			next_available_episode(&episodes, &episodes[0]),
			next_available_episode(&episodes, &episodes[1]),
			next_available_episode(&episodes, &episodes[2]),
		],
		[Some(&episodes[1]), Some(&episodes[2]), None],
	);
}
