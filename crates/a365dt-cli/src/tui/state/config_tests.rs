use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;

use super::{Change, ConfigView, Preference, items};
use crate::preferences::Preferences;

fn preferences() -> Preferences {
	Preferences {
		output: "/tmp/anime".into(),
		jobs: NonZeroUsize::new(4).unwrap(),
		mux: false,
		adult: false,
		adult_telemetry: false,
		auto_play_next_episode: false,
	}
}

#[test]
fn lists_every_editable_preference() {
	let view = ConfigView::new(preferences());

	assert_eq!(
		items(&view)
			.into_iter()
			.map(|item| (item.label, item.detail))
			.collect::<Vec<_>>(),
		vec![
			(
				"Output directory".into(),
				"/tmp/anime · Enter to change".into(),
			),
			("Concurrent downloads".into(), "4 · Enter to change".into(),),
			(
				"Mux separate subtitles".into(),
				"Disabled · Enter to change".into(),
			),
			("Adult content".into(), "Disabled · Enter to change".into(),),
			(
				"Adult telemetry detail".into(),
				"Disabled · Enter to change".into(),
			),
			(
				"Automatic next Episode".into(),
				"Disabled · Enter to change".into(),
			),
		],
	);
}

#[test]
fn toggles_boolean_preferences_and_waits_for_the_save_result() {
	let mut view = ConfigView::new(preferences());

	assert_eq!(view.activate(Preference::Mux), Some((1, Change::Mux(true))));
	assert_eq!(view.activate(Preference::Adult), None);
	let mut saved = preferences();
	saved.mux = true;
	view.finish(saved, "Saved".into());

	assert_eq!(
		view.activate(Preference::Mux),
		Some((2, Change::Mux(false)))
	);
}

#[test]
fn edits_text_preferences_and_keeps_invalid_jobs_open() {
	let mut view = ConfigView::new(preferences());

	assert_eq!(view.activate(Preference::Jobs), None);
	view.push('0');
	assert_eq!(view.submit(), None);
	assert!(view.editing());
	assert_eq!(
		view.message.as_deref(),
		Some("Enter a positive whole number.")
	);
	view.pop();
	view.push('8');
	assert_eq!(
		view.submit(),
		Some((1, Change::Jobs(NonZeroUsize::new(8).unwrap())))
	);

	view.finish(preferences(), "Saved".into());
	assert_eq!(view.activate(Preference::Output), None);
	for character in "~/Videos".chars() {
		view.push(character);
	}
	assert_eq!(view.submit(), Some((2, Change::Output("~/Videos".into()))));
}
