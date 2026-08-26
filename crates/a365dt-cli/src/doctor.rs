use std::process::ExitCode;

use indicatif::{HumanBytes, HumanDuration};

use crate::{
	cache::{Inspection as CacheInspection, MAX_AGE, Store},
	error::Error,
	playback,
	preferences::{self, Inspection as PreferencesInspection},
	startup::{self, Update},
	telemetry::{self, PerformanceMetric, Snapshot, Writer as TelemetryWriter},
};

mod report;
mod server;

pub(crate) use report::{Check, Status};
use report::{Report, Section};
use server::Probe as ServerProbe;

struct HealthInputs<'a> {
	server: &'a ServerProbe,
	community: &'a ServerProbe,
	h365: Option<&'a ServerProbe>,
	player: &'a Result<std::path::PathBuf, Error>,
	cache: &'a CacheInspection,
	snapshot: &'a Result<Snapshot, Error>,
	preferences: &'a Result<PreferencesInspection, Error>,
	debug: bool,
}

pub async fn run(
	store: &Store,
	telemetry_writer: &TelemetryWriter,
	debug: bool,
) -> ExitCode {
	let preferences =
		preferences::Store::discover().map(|store| store.inspect());
	let adult = adult_enabled(&preferences);
	let player = playback::inspect_player();
	let (server, community, h365, update, cache) = tokio::join!(
		server::probe(),
		server::probe_community(),
		async {
			if adult {
				Some(server::probe_h365().await)
			} else {
				None
			}
		},
		startup::check(store),
		store.inspect()
	);
	let telemetry = telemetry_writer.snapshot().await;
	let mut sections = vec![
		Section {
			title: "Health",
			debug: false,
			checks: health_checks(HealthInputs {
				server: &server,
				community: &community,
				h365: h365.as_ref(),
				player: &player,
				cache: &cache,
				snapshot: &telemetry,
				preferences: &preferences,
				debug,
			}),
		},
		Section {
			title: "Build",
			debug: false,
			checks: build_checks(&update),
		},
	];
	if debug {
		sections.push(Section {
			title: "Debug diagnostics",
			debug: true,
			checks: debug_checks(
				&server,
				&cache,
				&telemetry,
				&update,
				telemetry_writer,
				&preferences,
			),
		});
	}
	let report = Report { sections };
	report.print();
	report.exit_code()
}

fn health_checks(inputs: HealthInputs<'_>) -> Vec<Check> {
	let HealthInputs {
		server,
		community,
		h365,
		player,
		cache,
		snapshot,
		preferences,
		debug,
	} = inputs;
	let server = match server.status {
		Status::Error => Check::new("Anime365", &server.summary, server.status)
			.remedy("Check the network or Anime365 status, then retry"),
		Status::Warning => {
			Check::new("Anime365", &server.summary, server.status)
				.remedy("Retry; check the network if latency remains elevated")
		}
		Status::Healthy | Status::Info => {
			Check::new("Anime365", &server.summary, server.status)
		}
	};
	let h365 = h365.map(|probe| match probe.status {
		Status::Error => Check::new("H365", &probe.summary, probe.status)
			.remedy("Check H365 access or disable adult content"),
		Status::Warning => Check::new("H365", &probe.summary, probe.status)
			.remedy("Retry; H365 failures do not block Anime365"),
		Status::Healthy | Status::Info => {
			Check::new("H365", &probe.summary, probe.status)
		}
	});
	let community = match community.status {
		Status::Error => Check::new(
			"Anime365 community",
			&community.summary,
			Status::Warning,
		)
		.remedy("Moments and public profile enrichment may be unavailable; core search and playback are unaffected"),
		Status::Warning => Check::new(
			"Anime365 community",
			&community.summary,
			Status::Warning,
		),
		Status::Healthy | Status::Info => Check::new(
			"Anime365 community",
			&community.summary,
			community.status,
		),
	};
	let player = match player {
		Ok(path) => {
			Check::new("Player", path.display().to_string(), Status::Healthy)
		}
		Err(error) => Check::new("Player", error.message(), Status::Error)
			.remedy(if cfg!(target_os = "macos") {
				"Install IINA to enable Playback"
			} else {
				"Install mpv and add it to PATH to enable Playback"
			}),
	};
	let cache = match cache {
		CacheInspection::Ready { fresh: true, .. } => {
			Check::new("Series cache", "Fresh", Status::Healthy)
		}
		CacheInspection::Ready { .. } => {
			Check::new("Series cache", "Stale", Status::Warning)
				.remedy("Run a title search to refresh it")
		}
		CacheInspection::Unrefreshed { .. } => {
			Check::new("Series cache", "Not fully refreshed", Status::Warning)
				.remedy(
					"Run a title search and wait for the catalogue to refresh",
				)
		}
		CacheInspection::Missing { .. } => {
			Check::new("Series cache", "Not created yet", Status::Info)
				.remedy("Run a title search to create it")
		}
		CacheInspection::Broken { .. } => {
			Check::new("Series cache", "Unreadable", Status::Error)
				.remedy("Run `a365 cache prune` to reset it")
		}
	};
	let telemetry = match snapshot {
		Ok(snapshot) if snapshot.enabled => {
			Check::new("Local telemetry", "Enabled", Status::Healthy)
		}
		Ok(_) => Check::new("Local telemetry", "Disabled", Status::Warning)
			.remedy("Run `a365 telemetry enable` to resume observations"),
		Err(error) => {
			Check::new("Local telemetry", error.render(debug), Status::Error)
				.remedy("Run `a365 doctor --debug` to inspect its database")
		}
	};
	let preferences = preference_check(preferences, debug);
	let mut checks = vec![server, community];
	checks.extend(h365);
	checks.extend([player, cache, telemetry, preferences]);
	checks
}

fn adult_enabled(inspection: &Result<PreferencesInspection, Error>) -> bool {
	match inspection {
		Ok(PreferencesInspection::Missing { snapshot, .. })
		| Ok(PreferencesInspection::Ready { snapshot, .. }) => {
			snapshot.preferences.adult
		}
		Ok(PreferencesInspection::Invalid { .. })
		| Ok(PreferencesInspection::Unreadable { .. })
		| Err(_) => false,
	}
}

fn preference_check(
	inspection: &Result<PreferencesInspection, Error>,
	debug: bool,
) -> Check {
	match inspection {
		Ok(PreferencesInspection::Missing { .. }) => {
			Check::new("Preferences", "Built-in defaults", Status::Info)
		}
		Ok(PreferencesInspection::Ready { .. }) => {
			Check::new("Preferences", "Configured", Status::Healthy)
		}
		Ok(PreferencesInspection::Invalid { error, .. })
		| Ok(PreferencesInspection::Unreadable { error, .. })
		| Err(error) => Check::new("Preferences", error.render(debug), Status::Error)
			.remedy(
				"Run `a365 config` to repair or `a365 config reset` to remove it",
			),
	}
}

fn build_checks(update: &Result<Option<Update>, Error>) -> Vec<Check> {
	vec![
		version_check(update),
		Check::new("Commit", env!("A365_COMMIT_SHA"), Status::Info),
		Check::new("Profile", env!("A365_BUILD_PROFILE"), Status::Info),
		Check::new(
			"Platform",
			format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
			Status::Info,
		),
		Check::new("Compiler", env!("A365_RUSTC"), Status::Info),
	]
}

fn version_check(update: &Result<Option<Update>, Error>) -> Check {
	match update {
		Ok(Some(update)) => Check::new(
			"Version",
			format!("{} → {} available", update.installed, update.available),
			Status::Warning,
		)
		.remedy("Run `a365 update`"),
		Ok(None) => Check::new(
			"Version",
			concat!(env!("CARGO_PKG_VERSION"), " · up to date"),
			Status::Healthy,
		),
		Err(_) => Check::new(
			"Version",
			concat!(env!("CARGO_PKG_VERSION"), " · update check unavailable"),
			Status::Warning,
		)
		.remedy("Check the network or GitHub status, then retry"),
	}
}

fn debug_checks(
	server: &ServerProbe,
	cache: &CacheInspection,
	snapshot: &Result<Snapshot, Error>,
	update: &Result<Option<Update>, Error>,
	telemetry: &TelemetryWriter,
	preferences: &Result<PreferencesInspection, Error>,
) -> Vec<Check> {
	let mut checks = vec![
		Check::new("Server endpoint", server::URL, Status::Info),
		Check::new(
			"Server response",
			format!(
				"{} · {}",
				server.http_status.map_or_else(
					|| "No HTTP response".into(),
					|status| status.to_string()
				),
				milliseconds(server.latency.as_micros() as u64)
			),
			Status::Info,
		),
		Check::new(
			"Latency warning threshold",
			HumanDuration(server::LATENCY_WARNING).to_string(),
			Status::Info,
		),
	];
	if let Some(detail) = &server.detail {
		checks.push(Check::new("Server detail", detail, Status::Info));
	}
	if let Err(error) = update {
		checks.push(Check::new(
			"Update check detail",
			error.render(true),
			Status::Info,
		));
	}
	checks.extend(preference_debug_checks(preferences));
	let (cache_path, cache_detail) = match cache {
		CacheInspection::Ready {
			path, age, bytes, ..
		} => (
			path,
			format!(
				"{} old · TTL {} · {}",
				HumanDuration(*age),
				HumanDuration(MAX_AGE),
				HumanBytes(*bytes)
			),
		),
		CacheInspection::Unrefreshed {
			path,
			series,
			bytes,
		} => (
			path,
			format!(
				"Not fully refreshed · {series} Series · {}",
				HumanBytes(*bytes)
			),
		),
		CacheInspection::Missing { path, bytes } => {
			(path, format!("Missing · {}", HumanBytes(*bytes)))
		}
		CacheInspection::Broken {
			path,
			bytes,
			detail,
		} => (
			path,
			bytes.as_ref().map_or_else(
				|| detail.clone(),
				|bytes| format!("{detail} · {}", HumanBytes(*bytes)),
			),
		),
	};
	checks.push(Check::new(
		"Cache path",
		cache_path.display().to_string(),
		Status::Info,
	));
	checks.push(Check::new("Cache detail", cache_detail, Status::Info));
	match snapshot {
		Ok(snapshot) => {
			checks.extend([
				Check::new(
					"Telemetry data",
					format!(
						"{} · {}",
						snapshot.data_path.display(),
						snapshot.data_bytes.map_or_else(
							|| "missing".into(),
							|bytes| HumanBytes(bytes).to_string()
						)
					),
					Status::Info,
				),
				Check::new(
					"Telemetry opt-out",
					snapshot.disabled_path.display().to_string(),
					Status::Info,
				),
				Check::new(
					"Telemetry schema",
					snapshot.schema_version.to_string(),
					Status::Info,
				),
				Check::new(
					"First observation",
					telemetry::format_timestamp(snapshot.first_recorded_at),
					Status::Info,
				),
				Check::new(
					"Last observation",
					telemetry::format_timestamp(snapshot.last_recorded_at),
					Status::Info,
				),
				Check::new(
					"Last enabled",
					telemetry::format_timestamp(snapshot.last_enabled_at),
					Status::Info,
				),
				Check::new(
					"Last disabled",
					telemetry::format_timestamp(snapshot.last_disabled_at),
					Status::Info,
				),
				Check::new(
					"Last cleared",
					telemetry::format_timestamp(snapshot.last_cleared_at),
					Status::Info,
				),
			]);
			if snapshot.performance.is_empty() {
				checks.push(
					Check::new(
						"Per-operation latency",
						"Unavailable",
						Status::Info,
					)
					.remedy(
						"Collect telemetry by running searches or downloads",
					),
				);
			} else {
				checks.extend(snapshot.performance.iter().map(|metric| {
					Check::new(
						format!("Latency · {}", metric.operation),
						performance_detail(metric),
						Status::Info,
					)
				}));
			}
			if snapshot.counters.is_empty() {
				checks.push(
					Check::new("Usage counters", "Unavailable", Status::Info)
						.remedy("Run commands with telemetry enabled"),
				);
			} else {
				checks.extend(snapshot.counters.iter().map(
					|(counter, value)| {
						Check::new(
							format!("Counter · {counter}"),
							value.to_string(),
							Status::Info,
						)
					},
				));
			}
		}
		Err(error) => checks.push(Check::new(
			"Telemetry detail",
			error.render(true),
			Status::Error,
		)),
	}
	let overhead = telemetry.benchmark_overhead();
	checks.push(Check::new(
		"Telemetry overhead",
		format!(
			"enabled {} ns · disabled {} ns · added {} ns",
			overhead.enabled_ns, overhead.disabled_ns, overhead.added_ns
		),
		if overhead.added_ns <= 10_000 {
			Status::Healthy
		} else {
			Status::Warning
		},
	));
	checks
}

fn preference_debug_checks(
	inspection: &Result<PreferencesInspection, Error>,
) -> Vec<Check> {
	match inspection {
		Ok(PreferencesInspection::Missing { path, snapshot })
		| Ok(PreferencesInspection::Ready { path, snapshot }) => vec![
			Check::new("Config file", path.display().to_string(), Status::Info),
			Check::new(
				"Configured output",
				snapshot.preferences.output.display().to_string(),
				Status::Info,
			),
			Check::new(
				"Configured jobs",
				snapshot.preferences.jobs.to_string(),
				Status::Info,
			),
			Check::new(
				"Configured mux",
				if snapshot.preferences.mux {
					"Without confirmation"
				} else {
					"Ask"
				},
				Status::Info,
			),
			Check::new(
				"Adult content",
				enabled_label(snapshot.preferences.adult),
				Status::Info,
			),
			Check::new(
				"Adult telemetry detail",
				enabled_label(snapshot.preferences.adult_telemetry),
				Status::Info,
			),
			Check::new(
				"Automatic next Episode",
				enabled_label(snapshot.preferences.auto_play_next_episode),
				Status::Info,
			),
		],
		Ok(PreferencesInspection::Invalid { path, error })
		| Ok(PreferencesInspection::Unreadable { path, error }) => vec![
			Check::new("Config file", path.display().to_string(), Status::Info),
			Check::new("Config detail", error.message(), Status::Info),
		],
		Err(error) => {
			vec![Check::new("Config detail", error.message(), Status::Info)]
		}
	}
}

fn enabled_label(value: bool) -> &'static str {
	if value { "Enabled" } else { "Disabled" }
}

fn performance_detail(metric: &PerformanceMetric) -> String {
	let median = metric
		.samples_us
		.get(metric.samples_us.len() / 2)
		.copied()
		.unwrap_or_default();
	format!(
		"average {} · median {} · total {} · {} samples · {} work units",
		milliseconds(metric.total_us / metric.count.max(1)),
		milliseconds(median),
		milliseconds(metric.total_us),
		metric.samples_us.len(),
		metric.work_units
	)
}

fn milliseconds(microseconds: u64) -> String {
	format!("{:.3} ms", microseconds as f64 / 1_000.0)
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
