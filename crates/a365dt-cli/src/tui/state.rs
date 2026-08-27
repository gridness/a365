use crate::{
	anilist::{Library, ScheduleEntry, Viewer},
	api::{Embed, Profile, Translation},
	community::{MomentCategory, MomentPage, ProfileEnrichment},
	content::SeriesKey,
	continue_watching,
	series_search::Selection,
	telemetry::Recorder,
};

#[path = "state/config.rs"]
mod config;
#[path = "state/home.rs"]
mod home;
#[path = "state/items.rs"]
mod items;
#[path = "state/navigation.rs"]
mod navigation;
#[path = "state/search.rs"]
mod search;
#[path = "state/workflow.rs"]
mod workflow;

pub(crate) use config::{Change as ConfigChange, ConfigView};
pub(crate) use home::HomeView;
use search::RemoteSearch;
pub(crate) use search::{SeriesView, SourceWarning};
use workflow::{Choices, Workflow};
pub(crate) use workflow::{
	PlaybackSelection, Request as WorkflowRequest, ResumeSelection,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Destination {
	Home,
	Search,
	Timetable,
	Moments,
	AniList,
	Profile,
	Config,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Launch {
	Series(SeriesKey),
	ExternalSeries {
		my_anime_list_id: Option<u64>,
		anilist_id: u64,
		title: String,
	},
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
	pub home: HomeView,
	pub upgrade: Option<String>,
	pub debug: bool,
	pub series: Surface<SeriesView>,
	pub timetable: Surface<Vec<ScheduleEntry>>,
	pub moments: Surface<MomentPage>,
	pub anilist: Surface<AniListView>,
	pub profile: Surface<ProfileView>,
	pub config: ConfigView,
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
	ContinueWatching(continue_watching::Entry),
	OpenSeries { launch: Launch, title: String },
	Episode(u64),
	Track(crate::select::TrackKey),
	Playback(u16),
	ConnectAniList,
	Moment { id: u64, title: String },
	MomentCategory(Option<MomentCategory>),
	MomentPage(u32),
	Config(config::Preference),
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
	Search(String),
	Workflow {
		revision: u64,
		request: WorkflowRequest,
	},
	PlayMoment {
		id: u64,
		title: String,
	},
	ConnectAniList {
		revision: u64,
	},
	LoadMoments {
		page: u32,
		category: Option<MomentCategory>,
	},
	SaveConfig {
		revision: u64,
		change: ConfigChange,
	},
	Quit,
}

pub(crate) struct App {
	pub destination: Destination,
	pub query: String,
	pub selected: usize,
	pub viewport: usize,
	pub terminal_size: (u16, u16),
	data: Data,
	items: Vec<Item>,
	moment_category: Option<MomentCategory>,
	moment_page: u32,
	telemetry: Recorder,
	remote_search: Option<RemoteSearch>,
	workflow: Workflow,
	workflow_query: String,
	workflow_choices: Option<Choices>,
	workflow_revision: u64,
	anilist_connecting: bool,
	anilist_revision: u64,
}

impl Destination {
	pub(crate) const ALL: [Self; 7] = [
		Self::Home,
		Self::Search,
		Self::Timetable,
		Self::Moments,
		Self::AniList,
		Self::Profile,
		Self::Config,
	];

	pub(crate) const fn name(self) -> &'static str {
		match self {
			Self::Home => "Home",
			Self::Search => "Search",
			Self::Timetable => "Timetable",
			Self::Moments => "Moments",
			Self::AniList => "AniList",
			Self::Profile => "Profile",
			Self::Config => "Config",
		}
	}
}

impl App {
	pub(crate) fn new(
		destination: Destination,
		query: String,
		data: Data,
		telemetry: Recorder,
	) -> Self {
		let mut app = Self {
			destination,
			query,
			selected: 0,
			viewport: 0,
			terminal_size: (0, 0),
			data,
			items: Vec::new(),
			moment_category: None,
			moment_page: 1,
			telemetry,
			remote_search: None,
			workflow: Workflow::Browse,
			workflow_query: String::new(),
			workflow_choices: None,
			workflow_revision: 0,
			anilist_connecting: false,
			anilist_revision: 0,
		};
		app.rebuild();
		app
	}

	pub(crate) fn items(&self) -> &[Item] {
		&self.items
	}

	pub(crate) fn home_tip(&self) -> Option<&str> {
		(self.workflow.browsing() && self.destination == Destination::Home)
			.then_some(self.data.home.tip.as_deref())
			.flatten()
	}

	pub(crate) fn upgrade_notice(&self) -> Option<&str> {
		self.data.upgrade.as_deref()
	}

	pub(crate) const fn debug(&self) -> bool {
		self.data.debug
	}

	pub(crate) fn set_series(&mut self, series: Surface<SeriesView>) {
		self.data.series = series;
		self.rebuild();
	}

	pub(crate) fn set_continue_watching(
		&mut self,
		entry: Surface<continue_watching::Entry>,
	) {
		if let Surface::Ready(entry) = &entry {
			self.workflow.update_position(entry);
		}
		self.data.home.continue_watching = entry;
		self.rebuild();
	}

	pub(crate) fn set_trending_series(
		&mut self,
		series: Surface<Vec<crate::anilist::TrendingSeries>>,
	) {
		self.data.home.trending_series = series;
		self.rebuild();
	}

	pub(crate) fn set_trending_moments(
		&mut self,
		moments: Surface<MomentPage>,
	) {
		self.data.home.trending_moments = moments;
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
		self.anilist_connecting = false;
		self.data.anilist = anilist;
		self.rebuild();
	}

	pub(crate) fn set_profile(&mut self, profile: Surface<ProfileView>) {
		self.data.profile = profile;
		self.rebuild();
	}

	pub(crate) fn surface_message(&self) -> Option<&str> {
		if !self.workflow.browsing() {
			return self.workflow.message();
		}
		match self.destination {
			Destination::Home => None,
			Destination::Search => self.search_surface_message(),
			Destination::Timetable => surface_message(&self.data.timetable),
			Destination::Moments => surface_message(&self.data.moments),
			Destination::AniList if self.anilist_connecting => {
				Some("Waiting for browser approval…")
			}
			Destination::AniList
				if matches!(
					self.data.anilist,
					Surface::Empty | Surface::Error(_)
				) =>
			{
				None
			}
			Destination::AniList => surface_message(&self.data.anilist),
			Destination::Profile => surface_message(&self.data.profile),
			Destination::Config => None,
		}
	}

	pub(crate) fn profile(&self) -> Option<&ProfileView> {
		if !self.workflow.browsing() {
			return None;
		}
		match (&self.destination, &self.data.profile) {
			(Destination::Profile, Surface::Ready(profile)) => Some(profile),
			_ => None,
		}
	}

	pub(crate) fn anilist_viewer_name(&self) -> Option<&str> {
		if !self.workflow.browsing() {
			return None;
		}
		match (&self.destination, &self.data.anilist) {
			(Destination::AniList, Surface::Ready(view)) => {
				Some(&view.viewer.name)
			}
			_ => None,
		}
	}

	pub(crate) const fn shows_filter(&self) -> bool {
		self.workflow.filterable()
			|| (self.workflow.browsing()
				&& (matches!(self.destination, Destination::Search)
					|| (matches!(self.destination, Destination::Config)
						&& self.data.config.editing())))
	}

	pub(crate) fn filter_value(&self) -> &str {
		if self.workflow.filterable() {
			&self.workflow_query
		} else if self.destination == Destination::Config
			&& self.data.config.editing()
		{
			self.data.config.input()
		} else {
			&self.query
		}
	}

	pub(crate) fn filter_active(&self) -> bool {
		self.workflow.filterable()
			|| (self.workflow.browsing()
				&& (self.destination == Destination::Search
					|| (self.destination == Destination::Config
						&& self.data.config.editing())))
	}

	pub(crate) fn filter_placeholder(&self) -> String {
		if self.workflow.filterable() {
			self.workflow.filter_placeholder().into()
		} else if self.destination == Destination::Config {
			self.data.config.placeholder()
		} else {
			"Search title…".into()
		}
	}

	pub(crate) fn body_title(&self) -> String {
		self.workflow.title().unwrap_or_else(|| {
			self.anilist_viewer_name().map_or_else(
				|| self.destination.name().to_owned(),
				|name| format!("AniList · {name}"),
			)
		})
	}

	pub(crate) const fn browsing(&self) -> bool {
		self.workflow.browsing()
	}

	pub(crate) const fn loading(&self) -> bool {
		self.workflow.loading()
	}

	pub(crate) fn set_selection(&mut self, selection: Selection) {
		self.workflow.set_series(selection);
		self.prepare_workflow_choices();
	}

	pub(crate) fn set_translations(&mut self, translations: Vec<Translation>) {
		self.workflow.set_translations(translations);
		self.prepare_workflow_choices();
	}

	pub(crate) fn set_embed(&mut self, embed: Embed) {
		self.workflow.set_embed(embed);
		self.prepare_workflow_choices();
	}

	pub(crate) fn set_resume(&mut self, resume: ResumeSelection) {
		self.workflow.set_resume(resume);
		self.prepare_workflow_choices();
	}

	pub(crate) fn fail_workflow(&mut self, message: String) {
		self.workflow.fail(message);
		self.workflow_query.clear();
		self.workflow_choices = None;
		self.rebuild();
	}

	pub(super) fn prepare_workflow_choices(&mut self) {
		self.workflow_query.clear();
		self.workflow_choices = self.workflow.choices();
		self.selected = 0;
		self.viewport = 0;
		self.rebuild();
	}

	pub(crate) const fn workflow_revision(&self) -> u64 {
		self.workflow_revision
	}

	pub(crate) fn begin_anilist_connection(&mut self) -> u64 {
		self.anilist_revision = self.anilist_revision.wrapping_add(1);
		self.anilist_connecting = true;
		self.data.anilist = Surface::Loading;
		self.selected = 0;
		self.viewport = 0;
		self.rebuild();
		self.anilist_revision
	}

	pub(crate) const fn anilist_revision(&self) -> u64 {
		self.anilist_revision
	}

	pub(crate) fn playback(&self, height: u16) -> Option<PlaybackSelection> {
		self.workflow.playback(height)
	}

	pub(crate) fn begin_playback(&mut self, height: u16) -> bool {
		if !self.workflow.begin_playback(height) {
			return false;
		}
		self.workflow_choices = self.workflow.choices();
		self.rebuild();
		true
	}

	pub(crate) fn finish_playback(&mut self) {
		self.workflow.finish_playback();
		self.workflow_choices = self.workflow.choices();
		self.rebuild();
	}

	pub(crate) const fn playing(&self) -> bool {
		self.workflow.playing()
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
