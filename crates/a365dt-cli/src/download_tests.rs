use std::path::PathBuf;

use indicatif::{InMemoryTerm, ProgressDrawTarget};
use pretty_assertions::assert_eq;

use super::{Bars, Job, Outcome, Status, sanitize};
use crate::{
	api::{Episode, Translation},
	content::ContentSource,
	error::Error,
	select::PlannedRelease,
};

#[test]
fn creates_cross_platform_safe_names() {
	assert_eq!(sanitize("A/B: Team", 64), "A_B_ Team");
	assert_eq!(sanitize("CON", 64), "_CON");
}

#[test]
fn distinguishes_full_episode_labels_in_file_names() {
	let job = |id, episode_int: &str, episode_full: &str| Job {
		release: PlannedRelease {
			episode: Episode {
				source: ContentSource::Anime365,
				id,
				episode_int: episode_int.into(),
				episode_full: episode_full.into(),
			},
			translation: Translation {
				source: ContentSource::Anime365,
				id,
				episode_id: id,
				kind: "sub".into(),
				language: "ru".into(),
				authors_summary: "Team".into(),
			},
			height: 1080,
			media_url: String::new(),
			subtitle_url: None,
		},
		directory: PathBuf::new(),
		mux: false,
	};

	assert_eq!(
		[
			job(1, "5", "5 серия").stem(),
			job(2, "5", "TV SP 5 серия").stem(),
			job(3, "6.5", "6.5 серия").stem(),
		],
		[
			"E05 [sub-ru] [Team] [1080p]",
			"TV SP 5 [sub-ru] [Team] [1080p]",
			"E06.5 [sub-ru] [Team] [1080p]",
		]
	);
}

#[test]
fn leaves_completed_download_in_the_batch_as_a_green_checked_row() {
	let terminal = InMemoryTerm::new(4, 120);
	let debug = false;
	let mut bars = Bars::new(1, debug);
	bars.completed_style = bars.completed_style.clone().force_styling(true);
	bars.multi
		.set_draw_target(ProgressDrawTarget::term_like(Box::new(
			terminal.clone(),
		)));
	let bar = bars.transfer_bar("1 серия • 1080p");
	let outcome = Outcome {
		episode: "1 серия".into(),
		status: Status::Downloaded,
		bytes: 4,
		detail: Error::new("episode.mp4"),
	};

	bars.complete(&bar, &outcome);
	drop(bar);
	bars.overall.tick();

	let rendered = terminal.contents();
	let formatted = String::from_utf8(terminal.contents_formatted()).unwrap();
	let mut lines = rendered.lines();
	assert_eq!(
		(
			lines.next().is_some_and(|line| line.starts_with("Batch ")),
			lines.next(),
			lines.next(),
			formatted.lines().nth(1).is_some_and(|line| {
				line.starts_with(
					"\u{1b}[32m✓\u{1b}[m \u{1b}[32m1 серия • Completed\u{1b}[m",
				)
			}),
		),
		(true, Some("✓ 1 серия • Completed"), None, true)
	);
}
