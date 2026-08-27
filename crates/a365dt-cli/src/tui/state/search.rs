use crate::{api::Series, cache::Catalogue, content::ContentSource};

use super::{App, ItemAction, Launch, Surface};

#[derive(Clone, Debug)]
pub(crate) struct SeriesView {
	pub catalogue: Catalogue,
	pub warnings: Vec<SourceWarning>,
}

impl SeriesView {
	#[cfg(test)]
	pub(crate) fn new(
		series: Vec<Series>,
		warnings: Vec<SourceWarning>,
	) -> Self {
		Self::from_catalogue(Catalogue::new(series), warnings)
	}

	pub(crate) fn from_catalogue(
		catalogue: Catalogue,
		warnings: Vec<SourceWarning>,
	) -> Self {
		Self {
			catalogue,
			warnings,
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceWarning {
	pub source: ContentSource,
	pub message: String,
}

pub(super) struct RemoteSearch {
	pub(super) query: String,
	pub(super) series: Vec<Series>,
	pub(super) error: Option<String>,
}

impl App {
	pub(crate) fn set_remote_search(
		&mut self,
		query: String,
		series: Vec<Series>,
		error: Option<String>,
	) {
		if query != self.query {
			return;
		}
		self.remote_search = Some(RemoteSearch {
			query,
			series,
			error,
		});
		self.rebuild();
	}

	pub(super) fn search_surface_message(&self) -> Option<&str> {
		match &self.data.series {
			Surface::Loading => Some("Loading…"),
			Surface::Empty => Some("Nothing to show yet."),
			Surface::Error(error) => Some(error),
			Surface::Ready(_) if self.query.trim().is_empty() => {
				Some("Type a title or paste an official Anime365 URL.")
			}
			Surface::Ready(_)
				if !self.items.iter().any(|item| {
					matches!(
						item.action,
						ItemAction::OpenSeries {
							launch: Launch::Series(_),
							..
						}
					)
				}) =>
			{
				Some("No matching Series yet.")
			}
			Surface::Ready(_) => None,
		}
	}
}
