use pretty_assertions::assert_eq;

use super::state::{
	AniListView, App, Data, Destination, Event, Key, SeriesView, SourceWarning,
	Surface, Update,
};
use crate::{
	anilist::{
		Library, ListEntry, ListGroup, ListStatus, Media, MediaTitle,
		NextAiring, Viewer,
	},
	api::Series,
	community::{MomentCategory, MomentPage},
	content::ContentSource,
};

fn app(destination: Destination) -> App {
	App::new(
		destination,
		String::new(),
		Data {
			series: Surface::Ready(SeriesView {
				series: vec![Series {
					source: ContentSource::Anime365,
					id: 7,
					title: "Frieren".into(),
					year: Some(2023),
					type_title: Some("TV".into()),
					number_of_episodes: Some(28),
					my_anime_list_id: Some(52991),
					anilist_id: Some(154587),
					poster_url_small: None,
					episodes: Vec::new(),
				}],
				warnings: Vec::new(),
			}),
			timetable: Surface::Error("AniList unavailable".into()),
			moments: Surface::Empty,
			anilist: Surface::Error("Not connected".into()),
			profile: Surface::Error("Profile unavailable".into()),
		},
	)
}

#[test]
fn keyboard_navigation_and_search_are_pure_state_transitions() {
	let mut app = app(Destination::Home);

	assert_eq!(
		app.update(Event::Key(Key::Character('/'))),
		Update::Continue
	);
	assert_eq!(app.destination, Destination::Search);
	for character in "fri".chars() {
		app.update(Event::Key(Key::Character(character)));
	}

	assert_eq!((app.query.as_str(), app.items().len()), ("fri", 1));
	assert!(matches!(
		app.update(Event::Key(Key::Enter)),
		Update::Launch(_)
	));
}

#[test]
fn mouse_destinations_rows_and_wheel_use_the_same_update_path() {
	let mut app = app(Destination::Home);

	app.update(Event::ActivateDestination(Destination::Search));
	assert_eq!(app.destination, Destination::Search);
	assert!(matches!(
		app.update(Event::ActivateRow(0)),
		Update::Launch(_)
	));
	assert_eq!(app.update(Event::Scroll(3)), Update::Continue);
}

#[test]
fn one_failed_surface_does_not_hide_other_destinations() {
	let mut app = app(Destination::Timetable);
	assert_eq!(app.surface_message(), Some("AniList unavailable"));

	app.update(Event::ActivateDestination(Destination::Search));

	assert_eq!((app.surface_message(), app.items().len()), (None, 1));
}

#[test]
fn source_failure_warns_without_hiding_other_search_results() {
	let mut app = app(Destination::Search);
	app.set_series(Surface::Ready(SeriesView {
		series: vec![Series {
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
		warnings: vec![SourceWarning {
			source: ContentSource::H365,
			message: "request timed out".into(),
		}],
	}));

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
		Update::Launch(_)
	));
}

#[test]
fn only_content_launches_request_lazy_anime365_access() {
	use super::state::Launch;
	use crate::content::SeriesKey;

	assert_eq!(
		[
			Launch::Series(SeriesKey::new(ContentSource::Anime365, 1))
				.needs_content_sources(),
			Launch::ExternalSeries {
				my_anime_list_id: Some(1),
				anilist_id: 2,
				title: "Series".into(),
			}
			.needs_content_sources(),
			Launch::Moment {
				id: 3,
				title: "Moment".into(),
			}
			.needs_content_sources(),
		],
		[true, true, false],
	);
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

#[test]
fn anilist_filtering_searches_titles_lists_and_statuses() {
	let mut app = app(Destination::AniList);
	let entry = |id, title: &str, status| ListEntry {
		id,
		status,
		progress: 3,
		score: 88.0,
		priority: 7,
		media: Media {
			id,
			id_mal: Some(id + 100),
			is_adult: Some(false),
			title: MediaTitle {
				user_preferred: Some(title.into()),
				romaji: None,
				english: None,
				native: None,
			},
			next_airing_episode: Some(NextAiring {
				airing_at: 1_788_000_000,
				episode: 4,
			}),
		},
	};
	app.set_anilist(Surface::Ready(AniListView {
		viewer: Viewer {
			id: 1,
			name: "gridness".into(),
			avatar: None,
		},
		library: Library {
			lists: vec![
				ListGroup {
					name: "Watching".into(),
					is_custom_list: false,
					status: Some(ListStatus::Current),
					entries: vec![entry(
						1,
						"Frieren",
						Some(ListStatus::Current),
					)],
				},
				ListGroup {
					name: "Comfort shows".into(),
					is_custom_list: true,
					status: None,
					entries: vec![entry(2, "Orb", Some(ListStatus::Planning))],
				},
			],
		},
	}));

	assert_eq!(app.items().len(), 2);
	app.update(Event::Key(Key::Character('/')));
	for character in "comfort planning".chars() {
		app.update(Event::Key(Key::Character(character)));
	}

	assert_eq!(
		(
			app.anilist_query.as_str(),
			app.items().len(),
			app.items()[0].label(),
			app.items()[0].detail().contains("priority 7"),
			app.items()[0].detail().contains("next episode 4"),
		),
		("comfort planning", 1, "Orb", true, true),
	);
	assert_eq!(app.update(Event::Key(Key::Escape)), Update::Continue);
	assert_eq!((app.anilist_query.as_str(), app.items().len()), ("", 2));
}
