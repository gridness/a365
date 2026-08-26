use pretty_assertions::assert_eq;

use super::{
	Continuation, ContinuationAction, ContinuationStop, continuation_action,
	media_url_at_height,
};
use crate::{
	api::{Embed, MediaOption},
	playback::Outcome,
};

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
