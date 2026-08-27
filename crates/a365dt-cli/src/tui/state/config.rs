use std::num::NonZeroUsize;

use crate::preferences::Preferences;

use super::{App, Item, ItemAction, Surface};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Change {
	Output(String),
	Jobs(NonZeroUsize),
	Mux(bool),
	Adult(bool),
	AdultTelemetry(bool),
	AutoPlayNextEpisode(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Preference {
	Output,
	Jobs,
	Mux,
	Adult,
	AdultTelemetry,
	AutoPlayNextEpisode,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigView {
	preferences: Preferences,
	editing: Option<Preference>,
	input: String,
	message: Option<String>,
	saving: bool,
	revision: u64,
}

impl ConfigView {
	pub(crate) fn new(preferences: Preferences) -> Self {
		Self {
			preferences,
			editing: None,
			input: String::new(),
			message: None,
			saving: false,
			revision: 0,
		}
	}

	pub(super) const fn editing(&self) -> bool {
		self.editing.is_some()
	}

	pub(super) fn input(&self) -> &str {
		&self.input
	}

	pub(super) fn placeholder(&self) -> String {
		match self.editing {
			Some(Preference::Output) => format!(
				"New output directory (current: {})",
				self.preferences.output.display()
			),
			Some(Preference::Jobs) => format!(
				"Positive whole number (current: {})",
				self.preferences.jobs
			),
			Some(
				Preference::Mux
				| Preference::Adult
				| Preference::AdultTelemetry
				| Preference::AutoPlayNextEpisode,
			)
			| None => "Edit preference…".into(),
		}
	}

	pub(super) fn push(&mut self, character: char) {
		self.input.push(character);
	}

	pub(super) fn pop(&mut self) {
		self.input.pop();
	}

	pub(super) fn cancel_edit(&mut self) {
		self.editing = None;
		self.input.clear();
		self.message = None;
	}

	pub(super) fn activate(
		&mut self,
		preference: Preference,
	) -> Option<(u64, Change)> {
		if self.saving {
			return None;
		}
		self.message = None;
		match preference {
			Preference::Output | Preference::Jobs => {
				self.editing = Some(preference);
				self.input.clear();
				None
			}
			Preference::Mux => {
				self.begin_save(Change::Mux(!self.preferences.mux))
			}
			Preference::Adult => {
				self.begin_save(Change::Adult(!self.preferences.adult))
			}
			Preference::AdultTelemetry => self.begin_save(
				Change::AdultTelemetry(!self.preferences.adult_telemetry),
			),
			Preference::AutoPlayNextEpisode => {
				self.begin_save(Change::AutoPlayNextEpisode(
					!self.preferences.auto_play_next_episode,
				))
			}
		}
	}

	pub(super) fn submit(&mut self) -> Option<(u64, Change)> {
		let preference = self.editing?;
		let input = self.input.trim().to_owned();
		if input.is_empty() {
			self.cancel_edit();
			return None;
		}
		let change = match preference {
			Preference::Output => Change::Output(input),
			Preference::Jobs => match input.parse::<NonZeroUsize>() {
				Ok(jobs) => Change::Jobs(jobs),
				Err(_) => {
					self.message =
						Some("Enter a positive whole number.".into());
					return None;
				}
			},
			Preference::Mux
			| Preference::Adult
			| Preference::AdultTelemetry
			| Preference::AutoPlayNextEpisode => return None,
		};
		self.editing = None;
		self.input.clear();
		self.begin_save(change)
	}

	pub(crate) const fn revision(&self) -> u64 {
		self.revision
	}

	pub(crate) fn finish(&mut self, preferences: Preferences, message: String) {
		self.preferences = preferences;
		self.saving = false;
		self.message = Some(message);
	}

	pub(crate) fn fail(&mut self, message: String) {
		self.saving = false;
		self.message = Some(message);
	}

	fn begin_save(&mut self, change: Change) -> Option<(u64, Change)> {
		self.saving = true;
		self.message = Some("Saving preferences…".into());
		self.revision = self.revision.wrapping_add(1);
		Some((self.revision, change))
	}
}

impl App {
	pub(crate) fn finish_config(
		&mut self,
		preferences: Preferences,
		message: String,
	) {
		self.data.config.finish(preferences, message);
		self.rebuild();
	}

	pub(crate) fn fail_config(&mut self, message: String) {
		self.data.config.fail(message);
		self.rebuild();
	}

	pub(crate) const fn config_revision(&self) -> u64 {
		self.data.config.revision()
	}

	pub(crate) fn prepare_content_reload(&mut self) {
		self.data.series = Surface::Loading;
		self.data.timetable = Surface::Loading;
		self.data.moments = Surface::Loading;
		self.data.anilist = Surface::Loading;
		self.data.profile = Surface::Loading;
		self.rebuild();
	}
}

pub(super) fn items(view: &ConfigView) -> Vec<Item> {
	let mut items = vec![
		preference_item(
			"Output directory",
			view.preferences.output.display().to_string(),
			Preference::Output,
		),
		preference_item(
			"Concurrent downloads",
			view.preferences.jobs.to_string(),
			Preference::Jobs,
		),
		preference_item(
			"Mux separate subtitles",
			enabled(view.preferences.mux),
			Preference::Mux,
		),
		preference_item(
			"Adult content",
			enabled(view.preferences.adult),
			Preference::Adult,
		),
		preference_item(
			"Adult telemetry detail",
			enabled(view.preferences.adult_telemetry),
			Preference::AdultTelemetry,
		),
		preference_item(
			"Automatic next Episode",
			enabled(view.preferences.auto_play_next_episode),
			Preference::AutoPlayNextEpisode,
		),
	];
	if let Some(message) = &view.message {
		items.push(Item {
			label: message.clone(),
			detail: String::new(),
			action: ItemAction::None,
		});
	}
	items
}

fn preference_item(label: &str, value: String, preference: Preference) -> Item {
	Item {
		label: label.into(),
		detail: format!("{value} · Enter to change"),
		action: ItemAction::Config(preference),
	}
}

fn enabled(value: bool) -> String {
	if value { "Enabled" } else { "Disabled" }.into()
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
