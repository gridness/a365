use crate::{
	anilist::{Library, ScheduleEntry, Viewer},
	api::{Profile, Series},
	community::{MomentCategory, MomentPage, ProfileEnrichment},
	content::{ContentSource, SeriesKey},
};

#[path = "state/items.rs"]
mod items;

use items::{
	anilist_items, home_items, moment_items, search_items, timetable_items,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Destination {
	Home,
	Search,
	Timetable,
	Moments,
	AniList,
	Profile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Launch {
	Series(SeriesKey),
	ExternalSeries {
		my_anime_list_id: Option<u64>,
		anilist_id: u64,
		title: String,
	},
	Moment {
		id: u64,
		title: String,
	},
}

impl Launch {
	pub(crate) const fn needs_content_sources(&self) -> bool {
		matches!(self, Self::Series(_) | Self::ExternalSeries { .. })
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Surface<T> {
	Loading,
	Ready(T),
	Empty,
	Error(String),
}

#[derive(Clone, Debug)]
pub(crate) struct Data {
	pub series: Surface<SeriesView>,
	pub timetable: Surface<Vec<ScheduleEntry>>,
	pub moments: Surface<MomentPage>,
	pub anilist: Surface<AniListView>,
	pub profile: Surface<ProfileView>,
}

#[derive(Clone, Debug)]
pub(crate) struct SeriesView {
	pub series: Vec<Series>,
	pub warnings: Vec<SourceWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceWarning {
	pub source: ContentSource,
	pub message: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AniListView {
	pub viewer: Viewer,
	pub library: Library,
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileView {
	pub documented: Profile,
	pub enrichment: Result<ProfileEnrichment, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Item {
	pub label: String,
	pub detail: String,
	action: ItemAction,
}

impl Item {
	pub(crate) fn label(&self) -> &str {
		&self.label
	}

	pub(crate) fn detail(&self) -> &str {
		&self.detail
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ItemAction {
	None,
	Destination(Destination),
	Launch(Launch),
	MomentCategory(Option<MomentCategory>),
	MomentPage(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Key {
	Character(char),
	Backspace,
	Enter,
	Escape,
	Up,
	Down,
	Left,
	Right,
	Home,
	End,
	Tab,
	BackTab,
	Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Event {
	Key(Key),
	ActivateDestination(Destination),
	ActivateRow(usize),
	Scroll(i16),
	Resize { width: u16, height: u16 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Update {
	Continue,
	Launch(Launch),
	LoadMoments {
		page: u32,
		category: Option<MomentCategory>,
	},
	Quit,
}

pub(crate) struct App {
	pub destination: Destination,
	pub query: String,
	pub anilist_query: String,
	pub anilist_filtering: bool,
	pub selected: usize,
	pub viewport: usize,
	pub terminal_size: (u16, u16),
	data: Data,
	items: Vec<Item>,
	moment_category: Option<MomentCategory>,
	moment_page: u32,
}

impl Destination {
	pub(crate) const ALL: [Self; 6] = [
		Self::Home,
		Self::Search,
		Self::Timetable,
		Self::Moments,
		Self::AniList,
		Self::Profile,
	];

	pub(crate) const fn name(self) -> &'static str {
		match self {
			Self::Home => "Home",
			Self::Search => "Search",
			Self::Timetable => "Timetable",
			Self::Moments => "Moments",
			Self::AniList => "AniList",
			Self::Profile => "Profile",
		}
	}
}

impl App {
	pub(crate) fn new(
		destination: Destination,
		query: String,
		data: Data,
	) -> Self {
		let mut app = Self {
			destination,
			query,
			anilist_query: String::new(),
			anilist_filtering: false,
			selected: 0,
			viewport: 0,
			terminal_size: (0, 0),
			data,
			items: Vec::new(),
			moment_category: None,
			moment_page: 1,
		};
		app.rebuild();
		app
	}

	pub(crate) fn items(&self) -> &[Item] {
		&self.items
	}

	pub(crate) fn set_series(&mut self, series: Surface<SeriesView>) {
		self.data.series = series;
		self.rebuild();
	}

	pub(crate) fn set_timetable(
		&mut self,
		timetable: Surface<Vec<ScheduleEntry>>,
	) {
		self.data.timetable = timetable;
		self.rebuild();
	}

	pub(crate) fn set_moments(&mut self, moments: Surface<MomentPage>) {
		self.data.moments = moments;
		self.rebuild();
	}

	pub(crate) fn set_anilist(&mut self, anilist: Surface<AniListView>) {
		self.data.anilist = anilist;
		self.rebuild();
	}

	pub(crate) fn set_profile(&mut self, profile: Surface<ProfileView>) {
		self.data.profile = profile;
		self.rebuild();
	}

	pub(crate) fn surface_message(&self) -> Option<&str> {
		match self.destination {
			Destination::Home => None,
			Destination::Search => surface_message(&self.data.series),
			Destination::Timetable => surface_message(&self.data.timetable),
			Destination::Moments => surface_message(&self.data.moments),
			Destination::AniList => surface_message(&self.data.anilist),
			Destination::Profile => surface_message(&self.data.profile),
		}
	}

	pub(crate) fn profile(&self) -> Option<&ProfileView> {
		match (&self.destination, &self.data.profile) {
			(Destination::Profile, Surface::Ready(profile)) => Some(profile),
			_ => None,
		}
	}

	pub(crate) fn anilist_viewer_name(&self) -> Option<&str> {
		match (&self.destination, &self.data.anilist) {
			(Destination::AniList, Surface::Ready(view)) => {
				Some(&view.viewer.name)
			}
			_ => None,
		}
	}

	pub(crate) const fn shows_filter(&self) -> bool {
		matches!(self.destination, Destination::Search | Destination::AniList)
	}

	pub(crate) fn filter_value(&self) -> &str {
		if self.destination == Destination::AniList {
			&self.anilist_query
		} else {
			&self.query
		}
	}

	pub(crate) fn filter_active(&self) -> bool {
		self.destination == Destination::Search
			|| (self.destination == Destination::AniList
				&& self.anilist_filtering)
	}

	pub(crate) fn update(&mut self, event: Event) -> Update {
		match event {
			Event::Resize { width, height } => {
				self.terminal_size = (width, height);
			}
			Event::ActivateDestination(destination) => {
				self.activate(destination);
			}
			Event::ActivateRow(row) => {
				self.selected = row.min(self.items.len().saturating_sub(1));
				return self.activate_item();
			}
			Event::Scroll(delta) => self.scroll(delta),
			Event::Key(Key::Quit) => return Update::Quit,
			Event::Key(Key::Character('/'))
				if self.destination == Destination::AniList
					&& !self.anilist_filtering =>
			{
				self.anilist_filtering = true;
			}
			Event::Key(Key::Character(character)) => {
				if self.filter_active() && !character.is_control() {
					if self.destination == Destination::AniList {
						self.anilist_query.push(character);
					} else {
						self.query.push(character);
					}
					self.selected = 0;
					self.viewport = 0;
					self.rebuild();
				} else if let Some(destination) =
					destination_shortcut(character)
				{
					self.activate(destination);
				}
			}
			Event::Key(Key::Backspace) if self.filter_active() => {
				if self.destination == Destination::AniList {
					self.anilist_query.pop();
				} else {
					self.query.pop();
				}
				self.selected = 0;
				self.viewport = 0;
				self.rebuild();
			}
			Event::Key(Key::Enter) => return self.activate_item(),
			Event::Key(Key::Escape) => {
				if self.filter_active() && !self.filter_value().is_empty() {
					if self.destination == Destination::AniList {
						self.anilist_query.clear();
					} else {
						self.query.clear();
					}
					self.selected = 0;
					self.rebuild();
				} else if self.destination == Destination::AniList
					&& self.anilist_filtering
				{
					self.anilist_filtering = false;
				} else {
					return Update::Quit;
				}
			}
			Event::Key(Key::Up) => self.scroll(-1),
			Event::Key(Key::Down) => self.scroll(1),
			Event::Key(Key::Home) => self.selected = 0,
			Event::Key(Key::End) => {
				self.selected = self.items.len().saturating_sub(1);
			}
			Event::Key(Key::Left | Key::BackTab) => self.move_destination(-1),
			Event::Key(Key::Right | Key::Tab) => self.move_destination(1),
			Event::Key(Key::Backspace) => {}
		}
		Update::Continue
	}

	fn activate(&mut self, destination: Destination) {
		self.destination = destination;
		self.selected = 0;
		self.viewport = 0;
		self.rebuild();
	}

	fn move_destination(&mut self, delta: isize) {
		let index = Destination::ALL
			.iter()
			.position(|destination| *destination == self.destination)
			.unwrap_or_default();
		let count = Destination::ALL.len() as isize;
		let next = (index as isize + delta).rem_euclid(count) as usize;
		self.activate(Destination::ALL[next]);
	}

	fn scroll(&mut self, delta: i16) {
		if delta < 0 {
			self.selected =
				self.selected.saturating_sub(delta.unsigned_abs() as usize);
		} else {
			self.selected = (self.selected + delta as usize)
				.min(self.items.len().saturating_sub(1));
		}
	}

	fn activate_item(&mut self) -> Update {
		match self
			.items
			.get(self.selected)
			.map(|item| item.action.clone())
		{
			Some(ItemAction::None) => Update::Continue,
			Some(ItemAction::Destination(destination)) => {
				self.activate(destination);
				Update::Continue
			}
			Some(ItemAction::Launch(launch)) => Update::Launch(launch),
			Some(ItemAction::MomentCategory(category)) => {
				self.moment_category = category.clone();
				self.moment_page = 1;
				self.data.moments = Surface::Loading;
				self.rebuild();
				Update::LoadMoments { page: 1, category }
			}
			Some(ItemAction::MomentPage(page)) => {
				self.moment_page = page;
				self.data.moments = Surface::Loading;
				self.rebuild();
				Update::LoadMoments {
					page,
					category: self.moment_category.clone(),
				}
			}
			None => Update::Continue,
		}
	}

	fn rebuild(&mut self) {
		self.items = match self.destination {
			Destination::Home => home_items(&self.data),
			Destination::Search => search_items(&self.data.series, &self.query),
			Destination::Timetable => timetable_items(&self.data.timetable),
			Destination::Moments => moment_items(
				&self.data.moments,
				self.moment_category.as_ref(),
				self.moment_page,
			),
			Destination::AniList => {
				anilist_items(&self.data.anilist, &self.anilist_query)
			}
			Destination::Profile => Vec::new(),
		};
		self.selected = self.selected.min(self.items.len().saturating_sub(1));
	}
}

fn surface_message<T>(surface: &Surface<T>) -> Option<&str> {
	match surface {
		Surface::Loading => Some("Loading…"),
		Surface::Empty => Some("Nothing to show yet."),
		Surface::Error(error) => Some(error),
		Surface::Ready(_) => None,
	}
}

fn destination_shortcut(character: char) -> Option<Destination> {
	match character.to_ascii_lowercase() {
		'h' => Some(Destination::Home),
		'/' => Some(Destination::Search),
		't' => Some(Destination::Timetable),
		'm' => Some(Destination::Moments),
		'a' => Some(Destination::AniList),
		'p' => Some(Destination::Profile),
		_ => None,
	}
}
