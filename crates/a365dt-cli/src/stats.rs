use indicatif::HumanBytes;

use crate::{
	cache::{Inspection as CacheInspection, Store},
	doctor::{Check, Status},
	error::Error,
	telemetry::{self, Snapshot, Writer as TelemetryWriter},
	ui,
};

mod metrics;

use metrics::{Aggregate, aggregate};

pub async fn run(store: &Store, telemetry: &TelemetryWriter) {
	ui::heading("a365 stats");
	let cache = store.inspect().await;
	let telemetry = telemetry.snapshot().await;
	let rows = statistic_checks(&cache, &telemetry)
		.iter()
		.map(Check::row)
		.collect::<Vec<_>>();
	ui::grid(&rows);
}

fn statistic_checks(
	cache: &CacheInspection,
	snapshot: &Result<Snapshot, Error>,
) -> Vec<Check> {
	let mut checks = cache_statistics(cache);
	let Ok(snapshot) = snapshot else {
		for label in [
			"Catalogue hit rate",
			"API requests",
			"Media requests",
			"Cache retrieval",
			"Search",
			"Search throughput",
			"Downloads",
			"Download volume",
			"Command usage",
		] {
			checks.push(
				Check::new(label, "Unavailable", Status::Info).remedy(
					"Reset local telemetry and collect new observations",
				),
			);
		}
		return checks;
	};
	let suffix = if snapshot.enabled {
		""
	} else {
		" (historical)"
	};
	let hits = counter(snapshot, "catalogue.hits");
	let misses = counter(snapshot, "catalogue.misses");
	checks.push(rate_check("Catalogue hit rate", hits, misses, suffix));
	checks.push(performance_check(
		"API requests",
		aggregate(&snapshot.performance, "request.api."),
		suffix,
	));
	checks.push(performance_check(
		"Media requests",
		aggregate(&snapshot.performance, "request.asset."),
		suffix,
	));
	checks.push(performance_check(
		"Cache retrieval",
		aggregate(&snapshot.performance, "cache.retrieve"),
		suffix,
	));
	checks.push(performance_check(
		"Search",
		aggregate(&snapshot.performance, "search."),
		suffix,
	));
	let rank = aggregate(&snapshot.performance, "search.rank");
	checks.push(match rank {
		Some(metric) => Check::new(
			"Search throughput",
			{
				format!(
					"{:.0} Series/s{suffix}",
					metric.work_units as f64 * 1_000_000.0
						/ metric.total_us.max(1) as f64
				)
			},
			Status::Info,
		),
		None => Check::new(
			"Search throughput",
			"Unavailable (no observations)",
			Status::Info,
		)
		.remedy("Run searches with telemetry enabled"),
	});
	let downloaded = counter(snapshot, "downloads.episodes.downloaded");
	let skipped = counter(snapshot, "downloads.episodes.skipped");
	let failed = counter(snapshot, "downloads.episodes.failed")
		.saturating_add(counter(snapshot, "downloads.episodes.mux_failed"))
		.saturating_add(counter(snapshot, "downloads.episodes.interrupted"));
	checks.push(rate_check(
		"Downloads",
		downloaded.saturating_add(skipped),
		failed,
		suffix,
	));
	let batches = counter(snapshot, "downloads.batches");
	let bytes = counter(snapshot, "downloads.bytes");
	let episodes = downloaded.saturating_add(skipped).saturating_add(failed);
	checks.push(if batches == 0 {
		Check::new(
			"Download volume",
			"Unavailable (no observations)",
			Status::Info,
		)
		.remedy("Run downloads with telemetry enabled")
	} else {
		Check::new(
			"Download volume",
			format!(
				"{batches} batches · {episodes} Episodes · {}{suffix}",
				HumanBytes(bytes)
			),
			Status::Info,
		)
	});
	let commands = snapshot
		.counters
		.iter()
		.filter(|(key, _)| key.starts_with("commands."))
		.map(|(_, count)| count)
		.sum::<u64>();
	checks.push(Check::new(
		"Command usage",
		format!("{commands} commands{suffix}"),
		Status::Info,
	));
	checks
}

fn cache_statistics(cache: &CacheInspection) -> Vec<Check> {
	match cache {
		CacheInspection::Ready {
			path,
			refreshed_at,
			series,
			bytes,
			..
		} => vec![
			Check::new("Cache path", path.display().to_string(), Status::Info),
			Check::new(
				"Last cache update",
				telemetry::format_timestamp(Some(*refreshed_at)),
				Status::Info,
			),
			Check::new(
				"Cached Series",
				format!("{} · {}", series, HumanBytes(*bytes)),
				Status::Info,
			),
		],
		CacheInspection::Unrefreshed {
			path,
			series,
			bytes,
		} => vec![
			Check::new("Cache path", path.display().to_string(), Status::Info),
			Check::new(
				"Last cache update",
				"Never fully refreshed",
				Status::Info,
			)
			.remedy("Run a title search and wait for the catalogue to refresh"),
			Check::new(
				"Cached Series",
				format!("{series} · {}", HumanBytes(*bytes)),
				Status::Info,
			),
		],
		CacheInspection::Missing { path, bytes } => vec![
			Check::new("Cache path", path.display().to_string(), Status::Info),
			Check::new("Last cache update", "Never", Status::Info)
				.remedy("Run a title search to create the cache"),
			Check::new(
				"Cached Series",
				format!("0 · {}", HumanBytes(*bytes)),
				Status::Info,
			),
		],
		CacheInspection::Broken { path, bytes, .. } => vec![
			Check::new("Cache path", path.display().to_string(), Status::Info),
			Check::new("Last cache update", "Unavailable", Status::Info)
				.remedy("Run `a365 cache prune`"),
			Check::new(
				"Cached Series",
				bytes.map_or_else(
					|| "Unavailable".into(),
					|bytes| format!("Unavailable · {}", HumanBytes(bytes)),
				),
				Status::Info,
			)
			.remedy("Run `a365 cache prune`"),
		],
	}
}

fn performance_check(
	label: &str,
	metric: Option<Aggregate>,
	suffix: &str,
) -> Check {
	match metric {
		Some(metric) => Check::new(
			label,
			{
				format!(
					"average {} · median {} · {} observations{suffix}",
					milliseconds(metric.total_us / metric.count),
					milliseconds(metric.median_us),
					metric.count
				)
			},
			Status::Info,
		),
		None => {
			Check::new(label, "Unavailable (no observations)", Status::Info)
				.remedy("Run searches or downloads with telemetry enabled")
		}
	}
}

fn rate_check(label: &str, success: u64, failure: u64, suffix: &str) -> Check {
	let total = success.saturating_add(failure);
	if total == 0 {
		Check::new(label, "Unavailable (no observations)", Status::Info)
			.remedy("Run searches or downloads with telemetry enabled")
	} else {
		Check::new(
			label,
			{
				format!(
					"{:.1}% · {total} observations{suffix}",
					success as f64 / total as f64 * 100.0
				)
			},
			Status::Info,
		)
	}
}

fn counter(snapshot: &Snapshot, key: &str) -> u64 {
	snapshot.counters.get(key).copied().unwrap_or_default()
}

fn milliseconds(microseconds: u64) -> String {
	format!("{:.3} ms", microseconds as f64 / 1_000.0)
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod tests;
