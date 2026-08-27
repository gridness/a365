use pretty_assertions::assert_eq;

use super::{
	Presentation, TerminalMode, presentation, require_interactive_playback,
};
use crate::MediaAction;

#[test]
fn noninteractive_playback_fails_before_session_setup() {
	let playback = require_interactive_playback(
		MediaAction::Playback,
		TerminalMode::NonInteractive,
	)
	.map_err(|error| error.message().to_owned());
	let interactive_playback = require_interactive_playback(
		MediaAction::Playback,
		TerminalMode::Interactive,
	)
	.map_err(|error| error.message().to_owned());
	let download = require_interactive_playback(
		MediaAction::Download,
		TerminalMode::NonInteractive,
	)
	.map_err(|error| error.message().to_owned());

	assert_eq!(
		(playback, interactive_playback, download),
		(
			Err("Playback opens the full-screen TUI and requires an interactive terminal. Use `--download` for a non-interactive download flow.".to_owned()),
			Ok(()),
			Ok(()),
		),
	);
}

#[test]
fn normal_playback_is_quiet_while_debug_and_downloads_stay_detailed() {
	assert_eq!(
		[
			presentation(MediaAction::Playback, false),
			presentation(MediaAction::Playback, true),
			presentation(MediaAction::Download, false),
		],
		[
			Presentation::QuietTui,
			Presentation::Detailed,
			Presentation::Detailed,
		],
	);
}
