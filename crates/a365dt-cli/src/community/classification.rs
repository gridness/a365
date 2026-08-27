use std::collections::VecDeque;

use tokio::task::JoinSet;

use super::Moment;
use crate::api::{AdultClassification, Anime365};

const CONCURRENCY: usize = 8;
const HENTAI_MARKERS: [&str; 2] = ["hentai", "хентай"];

pub(super) async fn classify(moments: &mut [Moment], api: &Anime365) {
	let mut pending = VecDeque::new();
	for (index, moment) in moments.iter_mut().enumerate() {
		if explicit_adult_content(moment) {
			moment.is_adult = Some(true);
		} else if let Some(series_id) = moment.series_id {
			pending.push_back((index, series_id));
		}
	}

	let mut active = JoinSet::new();
	while active.len() < CONCURRENCY
		&& let Some(classification) = pending.pop_front()
	{
		spawn(&mut active, api.clone(), classification);
	}
	while let Some(joined) = active.join_next().await {
		if let Ok((index, classification)) = joined {
			moments[index].is_adult = match classification {
				AdultClassification::Adult => Some(true),
				AdultClassification::NonAdult => Some(false),
				AdultClassification::Unknown => None,
			};
		}
		if let Some(classification) = pending.pop_front() {
			spawn(&mut active, api.clone(), classification);
		}
	}
}

fn spawn(
	active: &mut JoinSet<(usize, AdultClassification)>,
	api: Anime365,
	(index, series_id): (usize, u64),
) {
	active.spawn(async move {
		let classification = api
			.adult_classification(series_id)
			.await
			.unwrap_or(AdultClassification::Unknown);
		(index, classification)
	});
}

fn explicit_adult_content(moment: &Moment) -> bool {
	let content = format!(
		"{} {}",
		moment.title,
		moment.episode.as_deref().unwrap_or_default()
	)
	.to_lowercase();
	HENTAI_MARKERS.iter().any(|marker| content.contains(marker))
}

#[cfg(test)]
#[path = "classification_tests.rs"]
mod tests;
