use crate::{
	api::{Embed, Episode, Series, Translation},
	content::ContentSource,
	continue_watching,
	playback::Position,
	search::Search,
	select::{PlannedRelease, TrackKey},
	series_search::Selection,
	telemetry::CatalogueUse,
};

use super::{Item, ItemAction, Launch};

pub(super) struct Choices {
	items: Vec<Item>,
	search: Search,
}

#[derive(Clone, Debug)]
pub(super) enum Workflow {
	Browse,
	Loading {
		title: String,
		message: String,
		back: Box<Self>,
		pending: Pending,
	},
	Episodes {
		series: Series,
		catalogue: CatalogueUse,
	},
	Translations {
		series: Series,
		catalogue: CatalogueUse,
		episode: Episode,
		translations: Vec<Translation>,
	},
	Resolutions {
		series: Series,
		catalogue: CatalogueUse,
		episode: Episode,
		translations: Vec<Translation>,
		track: TrackKey,
		translation: Translation,
		embed: Embed,
		playing: Option<u16>,
		position: Position,
	},
	Error {
		title: String,
		message: String,
		back: Box<Self>,
	},
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Request {
	Series(Launch),
	Resume(continue_watching::Entry),
	Translations {
		source: ContentSource,
		series_id: u64,
	},
	Media {
		source: ContentSource,
		translation_id: u64,
	},
	Playback(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlaybackSelection {
	pub series: Series,
	pub translations: Vec<Translation>,
	pub track: TrackKey,
	pub release: PlannedRelease,
	pub position: Position,
}

#[derive(Clone, Debug)]
pub(crate) struct ResumeSelection {
	pub series: Series,
	pub episode: Episode,
	pub translations: Vec<Translation>,
	pub track: TrackKey,
	pub translation: Translation,
	pub embed: Embed,
	pub height: u16,
	pub position: Position,
}

#[derive(Clone, Debug)]
pub(super) enum Pending {
	Series,
	Resume,
	Translations {
		series: Series,
		catalogue: CatalogueUse,
		episode: Episode,
	},
	Media {
		series: Series,
		catalogue: CatalogueUse,
		episode: Episode,
		translations: Vec<Translation>,
		track: TrackKey,
		translation: Translation,
	},
}

impl Workflow {
	pub(super) const fn browsing(&self) -> bool {
		matches!(self, Self::Browse)
	}

	pub(super) const fn filterable(&self) -> bool {
		matches!(
			self,
			Self::Episodes { .. }
				| Self::Translations { .. }
				| Self::Resolutions { .. }
		)
	}

	pub(super) const fn loading(&self) -> bool {
		matches!(self, Self::Loading { .. })
	}

	pub(super) const fn filter_placeholder(&self) -> &'static str {
		match self {
			Self::Episodes { .. } => "Search Episodes…",
			Self::Translations { .. } => "Search Translations…",
			Self::Resolutions { .. } => "Search resolutions…",
			Self::Browse | Self::Loading { .. } | Self::Error { .. } => {
				"Search…"
			}
		}
	}

	pub(super) fn begin_series(
		&mut self,
		launch: Launch,
		title: String,
	) -> Request {
		let back = Box::new(std::mem::replace(self, Self::Browse));
		*self = Self::Loading {
			title,
			message: "Loading Episodes…".into(),
			back,
			pending: Pending::Series,
		};
		Request::Series(launch)
	}

	pub(super) fn begin_resume(
		&mut self,
		entry: continue_watching::Entry,
	) -> Request {
		let back = Box::new(std::mem::replace(self, Self::Browse));
		*self = Self::Loading {
			title: format!(
				"Continue Watching · {} · Episode {}",
				entry.series_title, entry.episode_label
			),
			message: "Revalidating Translation and resolution…".into(),
			back,
			pending: Pending::Resume,
		};
		Request::Resume(entry)
	}

	pub(super) fn set_series(&mut self, selection: Selection) {
		*self = Self::Episodes {
			series: selection.series,
			catalogue: selection.catalogue,
		};
	}

	pub(super) fn begin_episode(&mut self, episode_id: u64) -> Option<Request> {
		let back = Box::new(self.clone());
		let Self::Episodes { series, catalogue } = self else {
			return None;
		};
		let episode = series
			.episodes
			.iter()
			.find(|episode| episode.id == episode_id)?
			.clone();
		let source = series.source;
		let series_id = series.id;
		let pending = Pending::Translations {
			series: series.clone(),
			catalogue: *catalogue,
			episode: episode.clone(),
		};
		*self = Self::Loading {
			title: format!(
				"{} · Episode {}",
				series.title, episode.episode_full
			),
			message: "Loading Translations…".into(),
			back,
			pending,
		};
		Some(Request::Translations { source, series_id })
	}

	pub(super) fn set_translations(&mut self, translations: Vec<Translation>) {
		let Self::Loading {
			pending:
				Pending::Translations {
					series,
					catalogue,
					episode,
				},
			..
		} = self
		else {
			return;
		};
		*self = Self::Translations {
			series: series.clone(),
			catalogue: *catalogue,
			episode: episode.clone(),
			translations,
		};
	}

	pub(super) fn begin_track(&mut self, track: TrackKey) -> Option<Request> {
		let back = Box::new(self.clone());
		let Self::Translations {
			series,
			catalogue,
			episode,
			translations,
		} = self
		else {
			return None;
		};
		let translation = crate::select::translation_for_track(
			translations,
			episode,
			&track,
		)?;
		let source = series.source;
		let translation_id = translation.id;
		let pending = Pending::Media {
			series: series.clone(),
			catalogue: *catalogue,
			episode: episode.clone(),
			translations: translations.clone(),
			track,
			translation,
		};
		*self = Self::Loading {
			title: format!(
				"{} · Episode {}",
				series.title, episode.episode_full
			),
			message: "Loading available media…".into(),
			back,
			pending,
		};
		Some(Request::Media {
			source,
			translation_id,
		})
	}

	pub(super) fn set_embed(&mut self, embed: Embed) {
		let Self::Loading {
			pending:
				Pending::Media {
					series,
					catalogue,
					episode,
					translations,
					track,
					translation,
				},
			..
		} = self
		else {
			return;
		};
		*self = Self::Resolutions {
			series: series.clone(),
			catalogue: *catalogue,
			episode: episode.clone(),
			translations: translations.clone(),
			track: track.clone(),
			translation: translation.clone(),
			embed,
			playing: None,
			position: Position::START,
		};
	}

	pub(super) fn set_resume(&mut self, resume: ResumeSelection) {
		if !matches!(
			self,
			Self::Loading {
				pending: Pending::Resume,
				..
			}
		) {
			return;
		}
		*self = Self::Resolutions {
			series: resume.series,
			catalogue: CatalogueUse::Hit,
			episode: resume.episode,
			translations: resume.translations,
			track: resume.track,
			translation: resume.translation,
			embed: resume.embed,
			playing: Some(resume.height),
			position: resume.position,
		};
	}

	pub(super) fn fail(&mut self, message: String) {
		let (title, back) = match std::mem::replace(self, Self::Browse) {
			Self::Loading { title, back, .. } => (title, back),
			other => ("Playback".into(), Box::new(other)),
		};
		*self = Self::Error {
			title,
			message,
			back,
		};
	}

	pub(super) fn back(&mut self) -> bool {
		let previous = match self {
			Self::Browse => return false,
			Self::Loading { back, .. } | Self::Error { back, .. } => {
				Some((**back).clone())
			}
			Self::Episodes { .. } => Some(Self::Browse),
			Self::Translations {
				series, catalogue, ..
			} => Some(Self::Episodes {
				series: series.clone(),
				catalogue: *catalogue,
			}),
			Self::Resolutions {
				series,
				catalogue,
				episode,
				translations,
				..
			} => Some(Self::Translations {
				series: series.clone(),
				catalogue: *catalogue,
				episode: episode.clone(),
				translations: translations.clone(),
			}),
		};
		if let Some(previous) = previous {
			*self = previous;
		}
		true
	}

	pub(super) fn title(&self) -> Option<String> {
		match self {
			Self::Browse => None,
			Self::Loading { title, .. } | Self::Error { title, .. } => {
				Some(title.clone())
			}
			Self::Episodes { series, .. } => {
				Some(format!("{} · Episodes", series.title))
			}
			Self::Translations {
				series, episode, ..
			} => Some(format!(
				"{} · Episode {} · Translations",
				series.title, episode.episode_full
			)),
			Self::Resolutions {
				series,
				episode,
				playing,
				..
			} => Some(if playing.is_some() {
				format!(
					"{} · Episode {} · Playing now in IINA",
					series.title, episode.episode_full
				)
			} else {
				format!(
					"{} · Episode {} · Resolution",
					series.title, episode.episode_full
				)
			}),
		}
	}

	pub(super) fn message(&self) -> Option<&str> {
		match self {
			Self::Loading { message, .. } | Self::Error { message, .. } => {
				Some(message)
			}
			Self::Browse
			| Self::Episodes { .. }
			| Self::Translations { .. }
			| Self::Resolutions { .. } => None,
		}
	}

	pub(super) fn choices(&self) -> Option<Choices> {
		if !self.filterable() {
			return None;
		}
		let items: Vec<Item> = match self {
			Self::Episodes { series, .. } => series
				.episodes
				.iter()
				.map(|episode| Item {
					label: format!("Episode {}", episode.episode_full),
					detail: "Select Translation".into(),
					action: ItemAction::Episode(episode.id),
				})
				.collect(),
			Self::Translations {
				episode,
				translations,
				..
			} => crate::select::tracks_for_episode(translations, episode)
				.into_iter()
				.map(|track| Item {
					label: format!("{}-{}", track.kind, track.language),
					detail: track.authors.clone(),
					action: ItemAction::Track(track),
				})
				.collect(),
			Self::Resolutions { embed, playing, .. } => {
				crate::select::available_heights(embed)
					.into_iter()
					.map(|height| Item {
						label: format!("{height}p"),
						detail: if *playing == Some(height) {
							"Playing now in IINA"
						} else {
							"Play in IINA"
						}
						.into(),
						action: ItemAction::Playback(height),
					})
					.collect()
			}
			Self::Browse | Self::Loading { .. } | Self::Error { .. } => {
				unreachable!("only stable workflow pages expose choices")
			}
		};
		let rows = items
			.iter()
			.map(|item| [item.label.clone(), item.detail.clone()])
			.collect::<Vec<_>>();
		Some(Choices {
			items,
			search: Search::new(&rows),
		})
	}

	pub(super) fn playback(&self, height: u16) -> Option<PlaybackSelection> {
		let Self::Resolutions {
			series,
			episode,
			translations,
			track,
			translation,
			embed,
			position,
			..
		} = self
		else {
			return None;
		};
		let media_url = embed
			.download
			.iter()
			.find(|option| option.height == height)
			.and_then(|option| option.url.clone())?;
		Some(PlaybackSelection {
			series: series.clone(),
			translations: translations.clone(),
			track: track.clone(),
			release: PlannedRelease {
				episode: episode.clone(),
				translation: translation.clone(),
				height,
				media_url,
				subtitle_url: embed.subtitles_url.clone(),
			},
			position: *position,
		})
	}

	pub(super) fn update_position(&mut self, entry: &continue_watching::Entry) {
		let Self::Resolutions {
			series,
			episode,
			track,
			position,
			..
		} = self
		else {
			return;
		};
		if series.key() == entry.series
			&& episode.id == entry.episode_id
			&& *track == entry.track
		{
			*position = entry.position;
		}
	}

	pub(super) fn begin_playback(&mut self, height: u16) -> bool {
		if self.playback(height).is_none() {
			return false;
		}
		let Self::Resolutions { playing, .. } = self else {
			return false;
		};
		*playing = Some(height);
		true
	}

	pub(super) fn finish_playback(&mut self) {
		if let Self::Resolutions { playing, .. } = self {
			*playing = None;
		}
	}

	pub(super) const fn playing(&self) -> bool {
		matches!(
			self,
			Self::Resolutions {
				playing: Some(_),
				..
			}
		)
	}
}

impl Choices {
	pub(super) fn searched(&self, query: &str) -> Vec<Item> {
		self.search
			.ranked(query)
			.into_iter()
			.map(|index| self.items[index].clone())
			.collect()
	}
}
