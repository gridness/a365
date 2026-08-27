use std::collections::HashSet;

use crate::{
	anilist::{ListStatus, ScheduleEntry},
	api::Series,
	community::{Moment, MomentCategory, MomentPage},
	telemetry::Recorder,
};

use super::{AniListView, Item, ItemAction, Launch, SeriesView, Surface};

pub(super) fn search_items(
	surface: &mut Surface<SeriesView>,
	query: &str,
	remote: Option<(&[Series], Option<&str>)>,
	telemetry: &Recorder,
) -> Vec<Item> {
	let Surface::Ready(view) = surface else {
		return Vec::new();
	};
	let mut items = view
		.warnings
		.iter()
		.map(|warning| Item {
			label: format!("{} unavailable", warning.source),
			detail: format!(
				"{} · continuing with other sources",
				warning.message
			),
			action: ItemAction::None,
		})
		.collect::<Vec<_>>();
	if let Some((_, Some(error))) = remote {
		items.push(Item {
			label: "Remote search unavailable".into(),
			detail: error.into(),
			action: ItemAction::None,
		});
	}
	if query.trim().is_empty() {
		return items;
	}
	let remote = remote.map(|(series, _)| series).unwrap_or_default();
	let remote_keys = remote.iter().map(Series::key).collect::<Vec<_>>();
	let mut seen = HashSet::new();
	let mut content = 0;
	for series in remote {
		if content == 200 {
			break;
		}
		if seen.insert(series.key()) {
			items.push(series_item(series));
			content += 1;
		}
	}
	let suggestions = view.catalogue.limited_suggestions(
		query,
		&remote_keys,
		telemetry,
		200_usize.saturating_sub(content),
	);
	for position in 0.. {
		if content == 200 {
			break;
		}
		let Some(series) = suggestions.series(position) else {
			break;
		};
		if seen.insert(series.key()) {
			items.push(series_item(series));
			content += 1;
		}
	}
	items
}

fn series_item(series: &Series) -> Item {
	Item {
		label: series.title.clone(),
		detail: format!(
			"{} · {} · {} episodes",
			series.source,
			series
				.year
				.map_or_else(|| "—".into(), |year| year.to_string()),
			series
				.number_of_episodes
				.map_or_else(|| "—".into(), |count| count.to_string()),
		),
		action: ItemAction::OpenSeries {
			launch: Launch::Series(series.key()),
			title: series.title.clone(),
		},
	}
}

pub(super) fn timetable_items(
	surface: &Surface<Vec<ScheduleEntry>>,
) -> Vec<Item> {
	match surface {
		Surface::Ready(entries) => entries.iter().map(schedule_item).collect(),
		Surface::Loading | Surface::Empty | Surface::Error(_) => Vec::new(),
	}
}

fn schedule_item(entry: &ScheduleEntry) -> Item {
	Item {
		label: entry.title.display().to_owned(),
		detail: format!(
			"Episode {} · {}",
			entry.episode,
			schedule_time(entry.airing_at)
		),
		action: ItemAction::OpenSeries {
			launch: Launch::ExternalSeries {
				my_anime_list_id: entry.mal_id,
				anilist_id: entry.media_id,
				title: entry.title.display().to_owned(),
			},
			title: entry.title.display().to_owned(),
		},
	}
}

pub(super) fn moment_items(
	surface: &Surface<MomentPage>,
	category: Option<&MomentCategory>,
	page_index: u32,
) -> Vec<Item> {
	let Surface::Ready(page) = surface else {
		return Vec::new();
	};
	let mut items = Vec::new();
	if category.is_some() {
		items.push(Item {
			label: "All Moments".into(),
			detail: "Category".into(),
			action: ItemAction::MomentCategory(None),
		});
	}
	items.extend(page.categories.iter().cloned().map(|category| Item {
		label: category.label.clone(),
		detail: "Category".into(),
		action: ItemAction::MomentCategory(Some(category)),
	}));
	if page_index > 1 {
		items.push(Item {
			label: "Previous page".into(),
			detail: format!("Moments page {page_index}"),
			action: ItemAction::MomentPage(page_index - 1),
		});
	}
	items.extend(page.moments.iter().map(moment_item));
	if let Some(next_page) = page.next_page {
		items.push(Item {
			label: "Next page".into(),
			detail: format!("Moments page {next_page}"),
			action: ItemAction::MomentPage(next_page),
		});
	}
	items
}

pub(super) fn moment_item(moment: &Moment) -> Item {
	let mut detail = [
		(!moment.duration.is_empty()).then_some(moment.duration.as_str()),
		moment.episode.as_deref(),
		moment.author.as_deref(),
		moment.age_or_date.as_deref(),
	]
	.into_iter()
	.flatten()
	.map(str::to_owned)
	.collect::<Vec<_>>();
	if let Some(views) = moment.views {
		detail.push(format!("{views} views"));
	}
	detail.push(format!("thumbnail {}", moment.thumbnail_url));
	Item {
		label: moment.title.clone(),
		detail: detail.join(" · "),
		action: ItemAction::Moment {
			id: moment.id,
			title: moment.title.clone(),
		},
	}
}

pub(super) fn anilist_items(surface: &Surface<AniListView>) -> Vec<Item> {
	let view = match surface {
		Surface::Ready(view) => view,
		Surface::Empty => {
			return vec![Item {
				label: "Connect AniList".into(),
				detail: "Open browser authorization".into(),
				action: ItemAction::ConnectAniList,
			}];
		}
		Surface::Error(error) => {
			return vec![Item {
				label: "Retry AniList connection".into(),
				detail: error.clone(),
				action: ItemAction::ConnectAniList,
			}];
		}
		Surface::Loading => return Vec::new(),
	};
	view.library
		.lists
		.iter()
		.flat_map(|list| {
			list.entries.iter().map(|entry| {
				let status = entry.status.map_or("Unlisted", ListStatus::name);
				let group = if list.is_custom_list {
					format!("{} (custom)", list.name)
				} else {
					list.name.clone()
				};
				let next =
					entry.media.next_airing_episode.as_ref().map_or_else(
						|| "no next airing".into(),
						|airing| {
							format!(
								"next episode {} · {}",
								airing.episode,
								schedule_time(airing.airing_at)
							)
						},
					);
				Item {
					label: entry.media.title.display().to_owned(),
					detail: format!(
						"{group} · {status} · progress {} · score {:.0} · priority {} · {next}",
						entry.progress, entry.score, entry.priority,
					),
					action: ItemAction::OpenSeries {
						launch: Launch::ExternalSeries {
							my_anime_list_id: entry.media.id_mal,
							anilist_id: entry.media.id,
							title: entry.media.title.display().to_owned(),
						},
						title: entry.media.title.display().to_owned(),
					},
				}
			})
		})
		.collect()
}

fn schedule_time(timestamp: i64) -> String {
	chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
		|| "unknown time".into(),
		|time| {
			time.with_timezone(&chrono::Local)
				.format("%a %H:%M")
				.to_string()
		},
	)
}
