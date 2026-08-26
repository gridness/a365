use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{
	api::{Embed, Episode, MediaOption, Translation},
	error::Error,
	ui,
};

#[derive(Clone, Debug, PartialEq)]
struct RangePlan {
	whole: Vec<Episode>,
	fractional: Vec<Episode>,
	missing: Vec<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TrackKey {
	pub kind: String,
	pub language: String,
	pub authors: String,
}

#[derive(Clone, Debug)]
struct Track {
	key: TrackKey,
	releases: HashMap<u64, Translation>,
}

#[derive(Clone, Debug)]
pub struct Release {
	pub episode: Episode,
	pub translation: Translation,
	pub embed: Embed,
}

#[derive(Clone, Debug)]
pub struct PlannedRelease {
	pub episode: Episode,
	pub translation: Translation,
	pub height: u16,
	pub media_url: String,
	pub subtitle_url: Option<String>,
}

pub fn choose_episodes(episodes: &[Episode]) -> Result<Vec<Episode>, Error> {
	loop {
		let input =
			ui::prompt("Episode ranges (examples: 1-12,16-18; 0-12.5; 5):")?;
		let plan = match plan_range(episodes, &input) {
			Ok(plan) => plan,
			Err(error) => {
				ui::warning(error);
				continue;
			}
		};
		if !plan.missing.is_empty() {
			ui::warning(format!(
				"Unavailable episodes: {}",
				comma(plan.missing.iter())
			));
			match ui::choose(
				"How should a365 proceed?",
				&[
					["Continue with available episodes".into()],
					["Enter a different selection".into()],
					["Cancel".into()],
				],
			)? {
				1 => continue,
				2 => return Err("Cancelled.".into()),
				_ => {}
			}
		}
		let mut selected = plan.whole;
		if !plan.fractional.is_empty() {
			let labels = plan
				.fractional
				.iter()
				.map(|episode| episode.episode_full.as_str());
			if ui::confirm(
				&format!("Include fractional episodes {}?", comma(labels)),
				false,
			)? {
				selected.extend(plan.fractional);
			}
		}
		selected.sort_by(episode_order);
		if selected.is_empty() {
			ui::warning("The selection contains no episodes.");
			continue;
		}
		return Ok(selected);
	}
}

pub fn choose_episode(episodes: &[Episode]) -> Result<Episode, Error> {
	if episodes.is_empty() {
		return Err("This Series has no Episodes.".into());
	}
	let rows = episodes
		.iter()
		.map(|episode| [episode.episode_full.clone()])
		.collect::<Vec<_>>();
	Ok(episodes[ui::choose("Episodes", &rows)?].clone())
}

pub fn translation_for_track(
	translations: &[Translation],
	episode: &Episode,
	track: &TrackKey,
) -> Option<Translation> {
	translations
		.iter()
		.filter(|translation| {
			translation.episode_id == episode.id
				&& translation.kind == track.kind
				&& translation.language == track.language
				&& translation.authors_summary == track.authors
		})
		.max_by_key(|translation| translation.id)
		.cloned()
}

pub fn choose_track(
	translations: Vec<Translation>,
	episodes: &[Episode],
) -> Result<(TrackKey, Vec<(Episode, Translation)>), Error> {
	let wanted: BTreeSet<_> =
		episodes.iter().map(|episode| episode.id).collect();
	let mut grouped: BTreeMap<TrackKey, HashMap<u64, Translation>> =
		BTreeMap::new();
	for translation in translations
		.into_iter()
		.filter(|translation| wanted.contains(&translation.episode_id))
	{
		let key = TrackKey {
			kind: translation.kind.clone(),
			language: translation.language.clone(),
			authors: translation.authors_summary.clone(),
		};
		grouped
			.entry(key)
			.or_default()
			.entry(translation.episode_id)
			.and_modify(|current| {
				if translation.id > current.id {
					*current = translation.clone();
				}
			})
			.or_insert(translation);
	}
	let mut tracks = grouped
		.into_iter()
		.map(|(key, releases)| Track { key, releases })
		.collect::<Vec<_>>();
	tracks.sort_by(|left, right| {
		right
			.releases
			.len()
			.cmp(&left.releases.len())
			.then_with(|| left.key.cmp(&right.key))
	});
	if tracks.is_empty() {
		return Err("No translations cover any selected episode.".into());
	}
	loop {
		let rows = tracks
			.iter()
			.map(|track| {
				let row = [
					format!("{}-{}", track.key.kind, track.key.language),
					track.key.authors.clone(),
					format!(
						"{}/{} episodes",
						track.releases.len(),
						episodes.len()
					),
				];
				if track.releases.len() == episodes.len() {
					row
				} else {
					row.map(ui::red)
				}
			})
			.collect::<Vec<_>>();
		let track = &tracks[ui::choose("Translation tracks", &rows)?];
		let missing = episodes
			.iter()
			.filter(|episode| !track.releases.contains_key(&episode.id))
			.collect::<Vec<_>>();
		if !missing.is_empty()
			&& !ui::confirm(
				&format!(
					"Download only available episodes and skip {}?",
					comma(
						missing
							.iter()
							.map(|episode| episode.episode_full.as_str())
					)
				),
				false,
			)? {
			continue;
		}
		let releases = episodes
			.iter()
			.filter_map(|episode| {
				track
					.releases
					.get(&episode.id)
					.cloned()
					.map(|translation| (episode.clone(), translation))
			})
			.collect();
		return Ok((track.key.clone(), releases));
	}
}

pub fn choose_resolutions(
	releases: Vec<Release>,
) -> Result<Vec<PlannedRelease>, Error> {
	let mut coverage: BTreeMap<u16, usize> = BTreeMap::new();
	for release in &releases {
		for height in heights(&release.embed) {
			*coverage.entry(height).or_default() += 1;
		}
	}
	if coverage.is_empty() {
		return Err("Anime365 returned no downloadable resolutions.".into());
	}
	let available = coverage.into_iter().rev().collect::<Vec<_>>();
	let rows = available
		.iter()
		.map(|(height, count)| {
			let row = [
				format!("{height}p"),
				format!("{count}/{} episodes", releases.len()),
			];
			if *count == releases.len() {
				row
			} else {
				row.map(ui::red)
			}
		})
		.collect::<Vec<_>>();
	let preferred = available[ui::choose("Preferred resolution", &rows)?].0;
	let mut chosen = vec![preferred; releases.len()];
	let mut fallback_groups: BTreeMap<Vec<u16>, Vec<usize>> = BTreeMap::new();
	for (index, release) in releases.iter().enumerate() {
		let options = heights(&release.embed);
		if !options.contains(&preferred) {
			fallback_groups.entry(options).or_default().push(index);
		}
	}
	for (options, indexes) in fallback_groups {
		if options.is_empty() {
			return Err(format!(
				"No downloadable resolution for episodes {}.",
				comma(indexes.iter().map(|index| {
					releases[*index].episode.episode_full.as_str()
				}))
			)
			.into());
		}
		let labels = options
			.iter()
			.map(|height| [format!("{height}p")])
			.collect::<Vec<_>>();
		let title = format!(
			"Fallback for episodes {}",
			comma(indexes.iter().map(|index| {
				releases[*index].episode.episode_full.as_str()
			}))
		);
		let height = options[ui::choose(&title, &labels)?];
		for index in indexes {
			chosen[index] = height;
		}
	}
	releases
		.into_iter()
		.zip(chosen)
		.map(|(release, height)| {
			let media_url = release
				.embed
				.download
				.iter()
				.find(|option| option.height == height)
				.and_then(|option| option.url.clone())
				.ok_or_else(|| {
					format!("Anime365 omitted the {height}p media URL")
				})?;
			Ok(PlannedRelease {
				episode: release.episode,
				translation: release.translation,
				height,
				media_url,
				subtitle_url: release.embed.subtitles_url,
			})
		})
		.collect()
}

fn plan_range(episodes: &[Episode], input: &str) -> Result<RangePlan, String> {
	let invalid_range = "Enter ascending ranges no wider than 10,000 episodes \
		after merging overlaps.";
	let mut ranges = input
		.split(',')
		.map(|input| {
			let (start, end) = if let Some((start, end)) = input.split_once('-')
			{
				(number(start)?, number(end)?)
			} else {
				let value = number(input)?;
				(value, value)
			};
			if start > end {
				return Err(invalid_range.into());
			}
			Ok((start, end))
		})
		.collect::<Result<Vec<_>, String>>()?;
	ranges.sort_by(|left, right| left.0.total_cmp(&right.0));
	let mut merged = Vec::<(f64, f64)>::new();
	for (start, end) in ranges {
		if let Some((_, previous_end)) = merged.last_mut()
			&& start <= *previous_end
		{
			*previous_end = previous_end.max(end);
		} else {
			merged.push((start, end));
		}
	}
	if merged.iter().any(|(start, end)| end - start > 10_000.0) {
		return Err(invalid_range.into());
	}
	let ranges = merged;
	let mut whole = Vec::new();
	let mut fractional = Vec::new();
	let mut present = BTreeSet::new();
	for episode in episodes {
		let Ok(value) = episode.episode_int.parse::<f64>() else {
			continue;
		};
		if ranges
			.iter()
			.any(|(start, end)| (*start..=*end).contains(&value))
		{
			if value.fract() == 0.0 {
				present.insert(value as u64);
				whole.push(episode.clone());
			} else {
				fractional.push(episode.clone());
			}
		}
	}
	let missing = ranges
		.into_iter()
		.flat_map(|(start, end)| start.ceil() as u64..=end.floor() as u64)
		.filter(|number| !present.contains(number))
		.collect();
	Ok(RangePlan {
		whole,
		fractional,
		missing,
	})
}

fn number(input: &str) -> Result<f64, String> {
	input
		.trim()
		.parse::<f64>()
		.ok()
		.filter(|number| number.is_finite() && *number >= 0.0)
		.ok_or_else(|| "Episode numbers must be non-negative numbers.".into())
}

fn heights(embed: &Embed) -> Vec<u16> {
	let mut heights = embed
		.download
		.iter()
		.filter_map(|option: &MediaOption| {
			option.url.as_ref().map(|_| option.height)
		})
		.collect::<Vec<_>>();
	heights.sort_unstable_by(|left, right| right.cmp(left));
	heights.dedup();
	heights
}

fn episode_order(left: &Episode, right: &Episode) -> std::cmp::Ordering {
	let left = left.episode_int.parse::<f64>().unwrap_or(f64::MAX);
	let right = right.episode_int.parse::<f64>().unwrap_or(f64::MAX);
	left.total_cmp(&right)
}

fn comma(items: impl IntoIterator<Item = impl std::fmt::Display>) -> String {
	items
		.into_iter()
		.map(|item| item.to_string())
		.collect::<Vec<_>>()
		.join(", ")
}

#[cfg(test)]
#[path = "select_tests.rs"]
mod tests;
