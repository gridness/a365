use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use ratatui::{Terminal, backend::TestBackend};

use super::state::{
	AniListView, App, ConfigView, Data, Destination, Event, HomeView, Key,
	ProfileView, SeriesView, SourceWarning, Surface, Update, WorkflowRequest,
};
use crate::{
	anilist::{Library, MediaTitle, TrendingSeries, Viewer},
	api::{Embed, Episode, MediaOption, Profile, Series, Translation},
	community::{Moment, MomentCategory, MomentPage, ProfileEnrichment},
	content::{ContentSource, SeriesKey},
	continue_watching,
	preferences::Preferences,
	select::TrackKey,
	series_search::Selection,
	telemetry::CatalogueUse,
};

fn test_series() -> Series {
	Series {
		source: ContentSource::Anime365,
		id: 7,
		title: "Frieren".into(),
		year: Some(2023),
		type_title: Some("TV".into()),
		number_of_episodes: Some(28),
		my_anime_list_id: Some(52991),
		anilist_id: Some(154587),
		poster_url_small: None,
		episodes: vec![
			Episode {
				source: ContentSource::Anime365,
				id: 70,
				episode_int: "1".into(),
				episode_full: "1".into(),
			},
			Episode {
				source: ContentSource::Anime365,
				id: 71,
				episode_int: "2".into(),
				episode_full: "2".into(),
			},
		],
	}
}

fn app(destination: Destination) -> App {
	app_with_options(destination, None, false)
}

fn app_with_upgrade(destination: Destination, upgrade: Option<String>) -> App {
	app_with_options(destination, upgrade, false)
}

fn app_with_options(
	destination: Destination,
	upgrade: Option<String>,
	debug: bool,
) -> App {
	App::new(
		destination,
		String::new(),
		Data {
			home: HomeView {
				tip: Some("Use a365 doctor to check the tool's health.".into()),
				continue_watching: Surface::Empty,
				trending_series: Surface::Empty,
				trending_moments: Surface::Empty,
			},
			upgrade,
			debug,
			series: Surface::Ready(SeriesView::new(
				vec![test_series()],
				Vec::new(),
			)),
			timetable: Surface::Error("AniList unavailable".into()),
			moments: Surface::Empty,
			anilist: Surface::Empty,
			profile: Surface::Error("Profile unavailable".into()),
			config: ConfigView::new(Preferences {
				output: "/tmp/a365-test-output".into(),
				jobs: NonZeroUsize::new(4).unwrap(),
				mux: false,
				adult: false,
				adult_telemetry: false,
				auto_play_next_episode: false,
			}),
		},
		Default::default(),
	)
}

fn render_screen(app: &mut App) -> Vec<String> {
	let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
	terminal
		.draw(|frame| {
			super::view::render(frame, app);
		})
		.unwrap();
	terminal
		.backend()
		.buffer()
		.content()
		.chunks(100)
		.map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
		.collect()
}

#[test]
fn home_renders_tip_above_selectable_content() {
	let mut app = app(Destination::Home);
	let screen = render_screen(&mut app);

	assert!(screen[3].contains("Tip  Use a365 doctor"));
	assert!(screen[6].contains("Continue Watching"));
	assert_eq!(app.items()[0].label(), "Continue Watching");
}

#[test]
fn available_upgrade_uses_the_first_footer_line_without_hiding_help() {
	let mut app = app_with_upgrade(
		Destination::Home,
		Some(
			"💫 Upgrade available · v3.0.0 → v3.1.0 · run `a365 update` for instructions"
				.into(),
		),
	);
	let screen = render_screen(&mut app);

	assert!(screen[22].contains("Upgrade available · v3.0.0 → v3.1.0"));
	assert!(screen[23].contains("Enter/click open"));
}

#[test]
fn profile_hides_technical_fields_unless_debug_is_enabled() {
	let profile = ProfileView {
		documented: Profile {
			is_logined: true,
			id: Some(42),
			name: Some("Test viewer".into()),
			is_premium: true,
			premium_until: Some("2027-01-01".into()),
		},
		enrichment: Ok(ProfileEnrichment {
			avatar_url: Some("https://example.com/avatar.jpg".into()),
			lists: Vec::new(),
			moments: Vec::new(),
		}),
	};
	let mut regular = app_with_options(Destination::Profile, None, false);
	regular.set_profile(Surface::Ready(profile.clone()));
	let regular_screen = render_screen(&mut regular).join("\n");
	let mut debug = app_with_options(Destination::Profile, None, true);
	debug.set_profile(Surface::Ready(profile));
	let debug_screen = render_screen(&mut debug).join("\n");

	assert!(regular_screen.contains("Account  Test viewer"));
	assert!(regular_screen.contains("Premium  yes · until 2027-01-01"));
	assert!(!regular_screen.contains("ID 42"));
	assert!(!regular_screen.contains("https://example.com/avatar.jpg"));
	assert!(debug_screen.contains("Account  Test viewer · ID 42"));
	assert!(debug_screen.contains("https://example.com/avatar.jpg"));
}

#[test]
fn profile_only_shows_enrichment_diagnostics_in_debug_mode() {
	let profile = ProfileView {
		documented: Profile {
			is_logined: true,
			id: None,
			name: Some("Test viewer".into()),
			is_premium: false,
			premium_until: None,
		},
		enrichment: Err("upstream parser rejected field 7".into()),
	};
	let mut regular = app_with_options(Destination::Profile, None, false);
	regular.set_profile(Surface::Ready(profile.clone()));
	let regular_screen = render_screen(&mut regular).join("\n");
	let mut debug = app_with_options(Destination::Profile, None, true);
	debug.set_profile(Surface::Ready(profile));
	let debug_screen = render_screen(&mut debug).join("\n");

	assert!(regular_screen.contains("Public profile enrichment unavailable"));
	assert!(!regular_screen.contains("upstream parser rejected field 7"));
	assert!(debug_screen.contains("upstream parser rejected field 7"));
}

#[test]
fn config_destination_edits_values_without_leaving_the_tui() {
	let mut app = app(Destination::Home);

	assert_eq!(
		app.update(Event::Key(Key::Character('c'))),
		Update::Continue
	);
	assert_eq!(app.destination, Destination::Config);
	assert_eq!(app.items().len(), 6);
	assert_eq!(app.update(Event::Key(Key::Enter)), Update::Continue);
	assert!(app.shows_filter());
	for character in "~/Videos".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::SaveConfig {
			revision: 1,
			change: super::state::ConfigChange::Output("~/Videos".into()),
		}
	);
}

#[test]
fn config_boolean_changes_are_saved_immediately() {
	let mut app = app(Destination::Config);

	assert_eq!(
		app.update(Event::ActivateRow(5)),
		Update::SaveConfig {
			revision: 1,
			change: super::state::ConfigChange::AutoPlayNextEpisode(true),
		}
	);
}

#[test]
fn home_orders_continue_watching_and_non_adult_playable_trends() {
	let mut app = app(Destination::Home);
	let mut adult_series = test_series();
	adult_series.source = ContentSource::H365;
	adult_series.id = 8;
	adult_series.title = "Adult title".into();
	adult_series.my_anime_list_id = Some(8_888);
	adult_series.anilist_id = Some(9_999);
	app.set_series(Surface::Ready(SeriesView::new(
		vec![test_series(), adult_series],
		Vec::new(),
	)));
	let entry = continue_watching::Entry {
		series: SeriesKey::new(ContentSource::Anime365, 7),
		series_title: "Frieren".into(),
		episode_id: 71,
		episode_label: "2".into(),
		track: TrackKey {
			kind: "sub".into(),
			language: "ru".into(),
			authors: "Team".into(),
		},
		height: 1080,
		position: crate::playback::Position::from_seconds(12 * 60 + 34),
	};
	app.set_continue_watching(Surface::Ready(entry.clone()));
	assert_eq!(
		app.items()[0].detail(),
		"Resume at 12:34 · Episode 2 · sub-ru by Team · 1080p",
	);
	app.set_trending_series(Surface::Ready(vec![
		TrendingSeries {
			id: 154_587,
			id_mal: Some(52_991),
			is_adult: Some(false),
			trending: 900,
			title: MediaTitle {
				user_preferred: Some("Frieren".into()),
				romaji: None,
				english: None,
				native: None,
			},
		},
		TrendingSeries {
			id: 9_999,
			id_mal: Some(8_888),
			is_adult: Some(false),
			trending: 800,
			title: MediaTitle {
				user_preferred: Some("Adult title".into()),
				romaji: None,
				english: None,
				native: None,
			},
		},
		TrendingSeries {
			id: 999,
			id_mal: None,
			is_adult: None,
			trending: 700,
			title: MediaTitle {
				user_preferred: Some("Unclassified".into()),
				romaji: None,
				english: None,
				native: None,
			},
		},
	]));
	app.set_trending_moments(Surface::Ready(MomentPage {
		moments: vec![
			Moment {
				id: 1,
				title: "Safe moment".into(),
				duration: "0:30".into(),
				thumbnail_url: "https://example.com/safe.jpg".into(),
				series_id: Some(7),
				episode: Some("Episode 2".into()),
				author: None,
				age_or_date: None,
				views: Some(10),
				is_adult: Some(false),
			},
			Moment {
				id: 2,
				title: "Unknown moment".into(),
				duration: "0:20".into(),
				thumbnail_url: "https://example.com/unknown.jpg".into(),
				series_id: None,
				episode: None,
				author: None,
				age_or_date: None,
				views: None,
				is_adult: None,
			},
		],
		categories: Vec::new(),
		next_page: None,
	}));

	assert_eq!(
		app.items()
			.iter()
			.map(|item| item.label())
			.collect::<Vec<_>>(),
		vec![
			"Continue Watching · Frieren",
			"Trending Series · Frieren",
			"Trending Moment · Safe moment",
			"Search Anime365 and H365",
		],
	);
	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			revision: 1,
			request: WorkflowRequest::Resume(entry.clone()),
		},
	);
}

#[test]
fn continue_watching_revalidates_and_resumes_the_exact_release() {
	let mut app = app(Destination::Home);
	let track = TrackKey {
		kind: "sub".into(),
		language: "ru".into(),
		authors: "Team".into(),
	};
	let entry = continue_watching::Entry {
		series: SeriesKey::new(ContentSource::Anime365, 7),
		series_title: "Frieren".into(),
		episode_id: 71,
		episode_label: "2".into(),
		track: track.clone(),
		height: 1080,
		position: crate::playback::Position::from_seconds(12 * 60 + 34),
	};
	app.set_continue_watching(Surface::Ready(entry.clone()));

	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			revision: 1,
			request: WorkflowRequest::Resume(entry.clone()),
		},
	);
	let translation = Translation {
		source: ContentSource::Anime365,
		id: 711,
		episode_id: 71,
		kind: "sub".into(),
		language: "ru".into(),
		authors_summary: "Team".into(),
	};
	let embed = Embed {
		download: vec![MediaOption {
			height: 1080,
			url: Some("https://media.example/video.mp4".into()),
		}],
		subtitles_url: Some("https://media.example/subtitles.ass".into()),
	};
	app.set_resume(super::state::ResumeSelection {
		series: test_series(),
		episode: test_series().episodes[1].clone(),
		translations: vec![translation.clone()],
		track: track.clone(),
		translation: translation.clone(),
		embed,
		height: 1080,
		position: crate::playback::Position::from_seconds(12 * 60 + 34),
	});

	assert_eq!(
		(app.body_title(), app.playing()),
		("Frieren · Episode 2 · Playing now in IINA".to_owned(), true,),
	);
	assert_eq!(
		app.playback(1080),
		Some(super::state::PlaybackSelection {
			series: test_series(),
			translations: vec![translation.clone()],
			track,
			release: crate::select::PlannedRelease {
				episode: test_series().episodes[1].clone(),
				translation,
				height: 1080,
				media_url: "https://media.example/video.mp4".into(),
				subtitle_url: Some(
					"https://media.example/subtitles.ass".into(),
				),
			},
			position: crate::playback::Position::from_seconds(12 * 60 + 34),
		}),
	);
	app.set_continue_watching(Surface::Ready(
		entry.with_position(crate::playback::Position::from_seconds(15 * 60)),
	));
	assert_eq!(
		app.playback(1080).unwrap().position,
		crate::playback::Position::from_seconds(15 * 60),
	);
}

#[test]
fn keyboard_navigation_and_search_are_pure_state_transitions() {
	let mut app = app(Destination::Home);
	assert_eq!(
		app.update(Event::Key(Key::Character('/'))),
		Update::Continue,
	);
	assert_eq!(app.destination, Destination::Search);

	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}

	assert_eq!((app.query.as_str(), app.items().len()), ("fri", 1));
	assert!(matches!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			request: WorkflowRequest::Series(_),
			..
		}
	));
}

#[test]
fn selecting_a_series_stays_in_the_tui_and_opens_its_episodes() {
	let mut app = app(Destination::Search);
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}

	let update = app.update(Event::Key(Key::Enter));
	let loading = (app.loading(), app.surface_message().map(str::to_owned));
	app.set_selection(Selection {
		series: test_series(),
		catalogue: CatalogueUse::Hit,
	});
	let rows = app
		.items()
		.iter()
		.map(|item| (item.label(), item.detail()))
		.collect::<Vec<_>>();

	assert!(matches!(
		update,
		Update::Workflow {
			request: WorkflowRequest::Series(_),
			..
		}
	));
	assert_eq!(loading, (true, Some("Loading Episodes…".to_owned())));
	assert_eq!(
		(app.surface_message(), rows),
		(
			None,
			vec![
				("Episode 1", "Select Translation"),
				("Episode 2", "Select Translation"),
			],
		),
	);
}

#[test]
fn nested_playback_choices_use_the_shared_search_behavior() {
	let mut app = app(Destination::Search);
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	app.update(Event::Key(Key::Enter));
	app.set_selection(Selection {
		series: test_series(),
		catalogue: CatalogueUse::Hit,
	});

	app.update(Event::Key(Key::Character('2')));

	assert_eq!(
		(
			app.shows_filter(),
			app.filter_value(),
			app.items()
				.iter()
				.map(|item| (item.label(), item.detail()))
				.collect::<Vec<_>>(),
		),
		(true, "2", vec![("Episode 2", "Select Translation")],),
	);

	assert!(matches!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			request: WorkflowRequest::Translations { series_id: 7, .. },
			..
		}
	));
	app.set_translations(vec![
		Translation {
			source: ContentSource::Anime365,
			id: 710,
			episode_id: 71,
			kind: "sub".into(),
			language: "ru".into(),
			authors_summary: "Subtitle Team".into(),
		},
		Translation {
			source: ContentSource::Anime365,
			id: 711,
			episode_id: 71,
			kind: "dub".into(),
			language: "ru".into(),
			authors_summary: "Voice Team".into(),
		},
	]);
	for character in "voce".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	assert_eq!(
		(
			app.filter_value(),
			app.items()
				.iter()
				.map(|item| (item.label(), item.detail()))
				.collect::<Vec<_>>(),
		),
		("voce", vec![("dub-ru", "Voice Team")]),
	);

	assert!(matches!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			request: WorkflowRequest::Media {
				translation_id: 711,
				..
			},
			..
		}
	));
	app.set_embed(Embed {
		download: vec![
			MediaOption {
				height: 1080,
				url: Some("https://media.example/1080.mp4".into()),
			},
			MediaOption {
				height: 720,
				url: Some("https://media.example/720.mp4".into()),
			},
		],
		subtitles_url: None,
	});
	for character in "108".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	assert_eq!(
		(
			app.filter_value(),
			app.items()
				.iter()
				.map(|item| (item.label(), item.detail()))
				.collect::<Vec<_>>(),
		),
		("108", vec![("1080p", "Play in IINA")]),
	);
}

#[test]
fn playback_handoff_preserves_the_nested_tui_state() {
	let mut app = app(Destination::Search);
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	app.update(Event::Key(Key::Enter));
	app.set_selection(Selection {
		series: test_series(),
		catalogue: CatalogueUse::Hit,
	});

	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			revision: 2,
			request: WorkflowRequest::Translations {
				source: ContentSource::Anime365,
				series_id: 7,
			},
		},
	);
	let translation = Translation {
		source: ContentSource::Anime365,
		id: 700,
		episode_id: 70,
		kind: "sub".into(),
		language: "ru".into(),
		authors_summary: "Team".into(),
	};
	app.set_translations(vec![translation.clone()]);
	assert_eq!(
		app.items()
			.iter()
			.map(|item| (item.label().to_owned(), item.detail().to_owned()))
			.collect::<Vec<_>>(),
		vec![("sub-ru".to_owned(), "Team".to_owned())],
	);
	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			revision: 3,
			request: WorkflowRequest::Media {
				source: ContentSource::Anime365,
				translation_id: 700,
			},
		},
	);
	app.set_embed(Embed {
		download: vec![MediaOption {
			height: 1080,
			url: Some("https://media.example/video.mp4".into()),
		}],
		subtitles_url: Some("https://media.example/subtitles.ass".into()),
	});
	let before = (
		app.surface_message().map(str::to_owned),
		app.body_title(),
		app.items()
			.iter()
			.map(|item| (item.label().to_owned(), item.detail().to_owned()))
			.collect::<Vec<_>>(),
	);

	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::Workflow {
			revision: 3,
			request: WorkflowRequest::Playback(1080),
		},
	);
	let during = (
		app.body_title(),
		app.items()
			.iter()
			.map(|item| (item.label().to_owned(), item.detail().to_owned()))
			.collect::<Vec<_>>(),
	);
	assert_eq!(
		during,
		(
			"Frieren · Episode 1 · Playing now in IINA".to_owned(),
			vec![("1080p".to_owned(), "Playing now in IINA".to_owned())],
		),
	);
	let playback = app.playback(1080).expect("1080p remains selectable");
	app.finish_playback();
	let after = (
		app.surface_message().map(str::to_owned),
		app.body_title(),
		app.items()
			.iter()
			.map(|item| (item.label().to_owned(), item.detail().to_owned()))
			.collect::<Vec<_>>(),
	);

	assert_eq!(
		playback,
		super::state::PlaybackSelection {
			series: test_series(),
			translations: vec![translation.clone()],
			track: crate::select::TrackKey {
				kind: translation.kind,
				language: translation.language,
				authors: translation.authors_summary,
			},
			release: crate::select::PlannedRelease {
				episode: test_series().episodes[0].clone(),
				translation: Translation {
					source: ContentSource::Anime365,
					id: 700,
					episode_id: 70,
					kind: "sub".into(),
					language: "ru".into(),
					authors_summary: "Team".into(),
				},
				height: 1080,
				media_url: "https://media.example/video.mp4".into(),
				subtitle_url: Some(
					"https://media.example/subtitles.ass".into(),
				),
			},
			position: crate::playback::Position::START,
		},
	);
	assert_eq!(after, before);
}

#[test]
fn series_search_matches_the_shared_typo_tolerant_ranking() {
	let mut app = app(Destination::Search);
	let mut update = Update::Continue;
	for character in "frieern".chars() {
		update = app.update(Event::Key(Key::Character(character)));
	}

	assert_eq!(update, Update::Search("frieern".into()));
	assert_eq!(
		app.items()
			.iter()
			.map(|item| item.label())
			.collect::<Vec<_>>(),
		vec!["Frieren"],
	);
}

#[test]
fn remote_enrichment_applies_only_to_the_current_query() {
	let mut app = app(Destination::Search);
	for character in "remote".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	let result = Series {
		source: ContentSource::Anime365,
		id: 8,
		title: "Remote result".into(),
		year: Some(2026),
		type_title: Some("ONA".into()),
		number_of_episodes: Some(1),
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: None,
		episodes: Vec::new(),
	};
	app.set_remote_search("remote".into(), vec![result.clone()], None);
	assert_eq!(
		app.items()
			.iter()
			.map(|item| item.label())
			.collect::<Vec<_>>(),
		vec!["Remote result"],
	);

	app.update(Event::Key(Key::Character('x')));
	app.set_remote_search("remote".into(), vec![result], None);
	assert_eq!(app.items().len(), 0);
}

#[test]
fn mouse_destinations_rows_and_wheel_use_the_same_update_path() {
	let mut app = app(Destination::Home);

	app.update(Event::ActivateDestination(Destination::Search));
	assert_eq!(app.destination, Destination::Search);
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}
	assert!(matches!(
		app.update(Event::ActivateRow(0)),
		Update::Workflow {
			request: WorkflowRequest::Series(_),
			..
		}
	));
	assert_eq!(app.update(Event::Scroll(3)), Update::Continue);
}

#[test]
fn one_failed_surface_does_not_hide_other_destinations() {
	let mut app = app(Destination::Timetable);
	assert_eq!(app.surface_message(), Some("AniList unavailable"));

	app.update(Event::ActivateDestination(Destination::Search));

	assert_eq!(
		(app.surface_message(), app.items().len()),
		(Some("Type a title or paste an official Anime365 URL."), 0,),
	);
}

#[test]
fn disconnected_anilist_connection_stays_inside_the_tui() {
	let mut app = app(Destination::AniList);

	assert_eq!(
		app.items()
			.iter()
			.map(|item| (item.label(), item.detail()))
			.collect::<Vec<_>>(),
		vec![("Connect AniList", "Open browser authorization")],
	);
	assert_eq!(
		app.update(Event::Key(Key::Enter)),
		Update::ConnectAniList { revision: 1 },
	);
	assert_eq!(
		(app.body_title(), app.surface_message(), app.items().len(),),
		(
			"AniList".to_owned(),
			Some("Waiting for browser approval…"),
			0,
		),
	);
}

#[test]
fn anilist_connection_result_replaces_waiting_with_library_or_retry() {
	let mut connected = app(Destination::AniList);
	connected.update(Event::Key(Key::Enter));
	connected.set_anilist(Surface::Ready(AniListView {
		viewer: Viewer {
			id: 1,
			name: "gridness".into(),
			avatar: None,
		},
		library: Library { lists: Vec::new() },
	}));

	assert_eq!(
		(
			connected.body_title(),
			connected.surface_message(),
			connected.items().len(),
		),
		("AniList · gridness".to_owned(), None, 0),
	);

	let mut failed = app(Destination::AniList);
	failed.update(Event::Key(Key::Enter));
	failed.set_anilist(Surface::Error(
		"AniList login was denied or cancelled in the browser.".into(),
	));
	assert_eq!(
		failed
			.items()
			.iter()
			.map(|item| (item.label(), item.detail()))
			.collect::<Vec<_>>(),
		vec![(
			"Retry AniList connection",
			"AniList login was denied or cancelled in the browser.",
		)],
	);
	assert_eq!(
		failed.update(Event::Key(Key::Enter)),
		Update::ConnectAniList { revision: 2 },
	);
}

#[test]
fn source_failure_warns_without_hiding_other_search_results() {
	let mut app = app(Destination::Search);
	app.set_series(Surface::Ready(SeriesView::new(
		vec![Series {
			source: ContentSource::Anime365,
			id: 7,
			title: "Frieren".into(),
			year: Some(2023),
			type_title: Some("TV".into()),
			number_of_episodes: Some(28),
			my_anime_list_id: Some(52_991),
			anilist_id: Some(154_587),
			poster_url_small: None,
			episodes: Vec::new(),
		}],
		vec![SourceWarning {
			source: ContentSource::H365,
			message: "request timed out".into(),
		}],
	)));
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}

	assert_eq!(
		app.items()
			.iter()
			.map(|item| (item.label(), item.detail()))
			.collect::<Vec<_>>(),
		vec![
			(
				"H365 unavailable",
				"request timed out · continuing with other sources",
			),
			("Frieren", "Anime365 · 2023 · 28 episodes"),
		],
	);
	assert_eq!(app.update(Event::ActivateRow(0)), Update::Continue);
	assert!(matches!(
		app.update(Event::ActivateRow(1)),
		Update::Workflow {
			request: WorkflowRequest::Series(_),
			..
		}
	));
}

#[test]
fn resize_empty_loading_error_and_cancellation_states_are_deterministic() {
	let mut app = app(Destination::Moments);
	assert_eq!(app.surface_message(), Some("Nothing to show yet."));
	app.set_moments(Surface::Loading);
	assert_eq!(app.surface_message(), Some("Loading…"));
	app.set_moments(Surface::Error("Markup changed".into()));
	assert_eq!(app.surface_message(), Some("Markup changed"));

	assert_eq!(
		app.update(Event::Resize {
			width: 120,
			height: 40,
		}),
		Update::Continue
	);
	assert_eq!(app.terminal_size, (120, 40));
	assert_eq!(app.update(Event::Key(Key::Quit)), Update::Quit);
}

#[test]
fn moments_categories_and_pagination_request_isolated_page_loads() {
	let mut app = app(Destination::Moments);
	let category = MomentCategory {
		label: "Recent".into(),
		id: "1".into(),
	};
	app.set_moments(Surface::Ready(MomentPage {
		moments: Vec::new(),
		categories: vec![category.clone()],
		next_page: Some(2),
	}));

	assert_eq!(
		app.update(Event::ActivateRow(0)),
		Update::LoadMoments {
			page: 1,
			category: Some(category.clone()),
		}
	);
	app.set_moments(Surface::Ready(MomentPage {
		moments: Vec::new(),
		categories: Vec::new(),
		next_page: Some(2),
	}));

	assert_eq!(
		app.update(Event::ActivateRow(1)),
		Update::LoadMoments {
			page: 2,
			category: Some(category),
		}
	);
}
