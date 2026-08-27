use std::collections::HashSet;

use crate::{
	anilist::TrendingSeries, community::MomentPage, content::ContentSource,
	continue_watching,
};

use super::{
	Data, Destination, Item, ItemAction, Launch, Surface, items::moment_item,
};

#[derive(Clone, Debug)]
pub(crate) struct HomeView {
	pub tip: Option<String>,
	pub continue_watching: Surface<continue_watching::Entry>,
	pub trending_series: Surface<Vec<TrendingSeries>>,
	pub trending_moments: Surface<MomentPage>,
}

impl HomeView {
	pub(crate) const fn loading(tip: Option<String>) -> Self {
		Self {
			tip,
			continue_watching: Surface::Loading,
			trending_series: Surface::Loading,
			trending_moments: Surface::Loading,
		}
	}
}

pub(super) fn home_items(data: &Data) -> Vec<Item> {
	let mut items = continue_watching_items(&data.home.continue_watching);
	items.extend(trending_series_items(
		&data.home.trending_series,
		&data.series,
	));
	items.extend(trending_moment_items(&data.home.trending_moments));
	items.push(Item {
		label: "Search Anime365 and H365".into(),
		detail: "Open Series search".into(),
		action: ItemAction::Destination(Destination::Search),
	});
	items
}

fn continue_watching_items(
	surface: &Surface<continue_watching::Entry>,
) -> Vec<Item> {
	match surface {
		Surface::Ready(entry) => vec![Item {
			label: format!("Continue Watching · {}", entry.series_title),
			detail: format!(
				"{}Episode {} · {}-{} by {} · {}p",
				if entry.position.at_start() {
					String::new()
				} else {
					format!("Resume at {} · ", entry.position)
				},
				entry.episode_label,
				entry.track.kind,
				entry.track.language,
				entry.track.authors,
				entry.height,
			),
			action: ItemAction::ContinueWatching(entry.clone()),
		}],
		Surface::Loading => vec![status_item("Continue Watching", "Loading…")],
		Surface::Empty => {
			vec![status_item("Continue Watching", "Nothing to resume yet")]
		}
		Surface::Error(error) => {
			vec![status_item("Continue Watching unavailable", error)]
		}
	}
}

fn trending_series_items(
	trends: &Surface<Vec<TrendingSeries>>,
	series: &Surface<super::SeriesView>,
) -> Vec<Item> {
	let Surface::Ready(trends) = trends else {
		return vec![surface_status("Trending Series", trends)];
	};
	let Surface::Ready(series) = series else {
		return vec![surface_status("Trending Series", series)];
	};
	let anime365 = HashSet::from([ContentSource::Anime365]);
	let mut seen = HashSet::new();
	let items = trends
		.iter()
		.filter(|trend| trend.is_adult == Some(false))
		.filter_map(|trend| {
			series
				.catalogue
				.external_match(trend.id_mal, trend.id, &anime365)
				.map(|series| (trend, series))
		})
		.filter(|(_, series)| seen.insert(series.key()))
		.take(5)
		.map(|(trend, series)| Item {
			label: format!("Trending Series · {}", series.title),
			detail: format!(
				"AniList trend {} · {} · {} episodes",
				trend.trending,
				series
					.year
					.map_or_else(|| "—".into(), |year| year.to_string()),
				series.number_of_episodes.map_or_else(
					|| "—".into(),
					|episodes| episodes.to_string(),
				),
			),
			action: ItemAction::OpenSeries {
				launch: Launch::Series(series.key()),
				title: series.title.clone(),
			},
		})
		.collect::<Vec<_>>();
	if items.is_empty() {
		vec![status_item(
			"Trending Series",
			"No current AniList trends are playable on Anime365",
		)]
	} else {
		items
	}
}

fn trending_moment_items(surface: &Surface<MomentPage>) -> Vec<Item> {
	let Surface::Ready(page) = surface else {
		return vec![surface_status("Trending Moments", surface)];
	};
	let items = page
		.moments
		.iter()
		.filter(|moment| moment.is_adult == Some(false))
		.take(5)
		.map(|moment| {
			let mut item = moment_item(moment);
			item.label = format!("Trending Moment · {}", item.label);
			item
		})
		.collect::<Vec<_>>();
	if items.is_empty() {
		vec![status_item(
			"Trending Moments",
			"No non-adult popular Moments are available",
		)]
	} else {
		items
	}
}

fn surface_status<T>(label: &str, surface: &Surface<T>) -> Item {
	match surface {
		Surface::Loading => status_item(label, "Loading…"),
		Surface::Empty => status_item(label, "Nothing to show yet"),
		Surface::Error(error) => {
			status_item(&format!("{label} unavailable"), error)
		}
		Surface::Ready(_) => {
			unreachable!("ready Home sections build content rows")
		}
	}
}

fn status_item(label: &str, detail: &str) -> Item {
	Item {
		label: label.into(),
		detail: detail.into(),
		action: ItemAction::None,
	}
}
