use super::{
	App, Destination, Event, ItemAction, Key, Surface, Update, WorkflowRequest,
	config,
	home::home_items,
	items::{anilist_items, moment_items, search_items, timetable_items},
	workflow::Workflow,
};

impl App {
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
			Event::Key(Key::Character(character)) => {
				if self.filter_active() && !character.is_control() {
					if self.workflow.filterable() {
						self.workflow_query.push(character);
					} else if self.destination == Destination::Config {
						self.data.config.push(character);
					} else {
						self.query.push(character);
					}
					self.selected = 0;
					self.viewport = 0;
					self.rebuild();
					if self.workflow.browsing()
						&& self.destination == Destination::Search
					{
						return Update::Search(self.query.clone());
					}
				} else if let Some(destination) =
					destination_shortcut(character)
				{
					self.activate(destination);
				}
			}
			Event::Key(Key::Backspace) if self.filter_active() => {
				if self.workflow.filterable() {
					self.workflow_query.pop();
				} else if self.destination == Destination::Config {
					self.data.config.pop();
				} else {
					self.query.pop();
				}
				self.selected = 0;
				self.viewport = 0;
				self.rebuild();
				if self.workflow.browsing()
					&& self.destination == Destination::Search
				{
					return Update::Search(self.query.clone());
				}
			}
			Event::Key(Key::Enter)
				if self.destination == Destination::Config
					&& self.data.config.editing() =>
			{
				let update = self.data.config.submit();
				return self.config_update(update);
			}
			Event::Key(Key::Enter) => return self.activate_item(),
			Event::Key(Key::Escape) => {
				if self.destination == Destination::Config
					&& self.data.config.editing()
				{
					self.data.config.cancel_edit();
					self.rebuild();
					return Update::Continue;
				} else if self.workflow.filterable()
					&& !self.workflow_query.is_empty()
				{
					self.workflow_query.clear();
					self.selected = 0;
					self.viewport = 0;
					self.rebuild();
					return Update::Continue;
				} else if self.workflow.back() {
					self.workflow_revision =
						self.workflow_revision.wrapping_add(1);
					self.prepare_workflow_choices();
					return Update::Continue;
				} else if self.filter_active()
					&& !self.filter_value().is_empty()
				{
					self.query.clear();
					self.selected = 0;
					self.rebuild();
					if self.destination == Destination::Search {
						return Update::Search(self.query.clone());
					}
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
		self.data.config.cancel_edit();
		self.workflow = Workflow::Browse;
		self.workflow_query.clear();
		self.workflow_choices = None;
		self.workflow_revision = self.workflow_revision.wrapping_add(1);
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
			Some(ItemAction::ContinueWatching(entry)) => {
				let request = self.workflow.begin_resume(entry);
				self.start_workflow(request)
			}
			Some(ItemAction::OpenSeries { launch, title }) => {
				let request = self.workflow.begin_series(launch, title);
				self.start_workflow(request)
			}
			Some(ItemAction::Episode(episode_id)) => {
				match self.workflow.begin_episode(episode_id) {
					Some(request) => self.start_workflow(request),
					None => Update::Continue,
				}
			}
			Some(ItemAction::Track(track)) => {
				match self.workflow.begin_track(track) {
					Some(request) => self.start_workflow(request),
					None => Update::Continue,
				}
			}
			Some(ItemAction::Playback(height))
				if self.begin_playback(height) =>
			{
				Update::Workflow {
					revision: self.workflow_revision,
					request: WorkflowRequest::Playback(height),
				}
			}
			Some(ItemAction::Playback(_)) => Update::Continue,
			Some(ItemAction::ConnectAniList) => Update::ConnectAniList {
				revision: self.begin_anilist_connection(),
			},
			Some(ItemAction::Moment { id, title }) => {
				Update::PlayMoment { id, title }
			}
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
			Some(ItemAction::Config(preference)) => {
				let update = self.data.config.activate(preference);
				self.rebuild();
				self.config_update(update)
			}
			None => Update::Continue,
		}
	}

	fn config_update(
		&mut self,
		update: Option<(u64, config::Change)>,
	) -> Update {
		match update {
			Some((revision, change)) => {
				self.rebuild();
				Update::SaveConfig { revision, change }
			}
			None => {
				self.rebuild();
				Update::Continue
			}
		}
	}

	fn start_workflow(&mut self, request: WorkflowRequest) -> Update {
		self.workflow_query.clear();
		self.workflow_choices = None;
		self.selected = 0;
		self.viewport = 0;
		self.rebuild();
		self.workflow_update(request)
	}

	fn workflow_update(&mut self, request: WorkflowRequest) -> Update {
		self.workflow_revision = self.workflow_revision.wrapping_add(1);
		Update::Workflow {
			revision: self.workflow_revision,
			request,
		}
	}

	pub(super) fn rebuild(&mut self) {
		if !self.workflow.browsing() {
			self.items = self
				.workflow_choices
				.as_ref()
				.map(|choices| choices.searched(&self.workflow_query))
				.unwrap_or_default();
			self.selected =
				self.selected.min(self.items.len().saturating_sub(1));
			return;
		}
		self.items = match self.destination {
			Destination::Home => home_items(&self.data),
			Destination::Search => {
				let remote = self
					.remote_search
					.as_ref()
					.filter(|remote| remote.query == self.query)
					.map(|remote| {
						(remote.series.as_slice(), remote.error.as_deref())
					});
				search_items(
					&mut self.data.series,
					&self.query,
					remote,
					&self.telemetry,
				)
			}
			Destination::Timetable => timetable_items(&self.data.timetable),
			Destination::Moments => moment_items(
				&self.data.moments,
				self.moment_category.as_ref(),
				self.moment_page,
			),
			Destination::AniList => anilist_items(&self.data.anilist),
			Destination::Profile => Vec::new(),
			Destination::Config => config::items(&self.data.config),
		};
		self.selected = self.selected.min(self.items.len().saturating_sub(1));
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
		'c' => Some(Destination::Config),
		_ => None,
	}
}
