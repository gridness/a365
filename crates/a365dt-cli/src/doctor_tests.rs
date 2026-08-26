use pretty_assertions::assert_eq;
use semver::Version;

use super::{Check, Report, Section, Status, preference_check, version_check};
use crate::{
	error::Error, preferences::Inspection as PreferencesInspection,
	startup::Update,
};

#[test]
fn reports_the_worst_health_status() {
	let report = Report {
		sections: vec![Section {
			title: "Health",
			debug: false,
			checks: vec![
				Check::new("API", "Available", Status::Healthy),
				Check::new("Cache", "Stale", Status::Warning),
				Check::new("Telemetry", "Unavailable", Status::Error),
			],
		}],
	};

	assert_eq!(report.status(), Status::Error);
}

#[test]
fn reports_current_available_and_unknown_versions() {
	let installed = env!("CARGO_PKG_VERSION");

	assert_eq!(
		[
			version_check(&Ok(None)),
			version_check(&Ok(Some(Update {
				installed: Version::new(0, 9, 0),
				available: Version::new(0, 10, 0),
				release_url: "https://example.com/release".into(),
			}))),
			version_check(&Err(Error::new("unavailable"))),
		],
		[
			Check::new(
				"Version",
				format!("{installed} · up to date"),
				Status::Healthy,
			),
			Check::new("Version", "0.9.0 → 0.10.0 available", Status::Warning,)
				.remedy("Run `a365 update`"),
			Check::new(
				"Version",
				format!("{installed} · update check unavailable"),
				Status::Warning,
			)
			.remedy("Check the network or GitHub status, then retry"),
		]
	);
}

#[test]
fn invalid_preferences_make_doctor_unhealthy() {
	let error = Error::new("unknown field `job`");

	assert_eq!(
		preference_check(
			&Ok(PreferencesInspection::Invalid {
				path: "/home/me/.a365/config.toml".into(),
				error: error.clone(),
			}),
			false,
		),
		Check::new("Preferences", error.message(), Status::Error).remedy(
			"Run `a365 config` to repair or `a365 config reset` to remove it",
		)
	);
}

#[test]
fn community_probe_distinguishes_reachability_from_broken_markup() {
	use super::server::{ResponseContract, contract_error};

	let reachable = b"<main>HTTP success without a Moments feed</main>";
	let valid = br#"<div data-moments-list data-total="0"></div>"#;

	assert_eq!(
		[
			contract_error(ResponseContract::Reachable, reachable),
			contract_error(ResponseContract::MomentsMarkup, valid),
		],
		[None, None],
	);
	assert!(
		contract_error(ResponseContract::MomentsMarkup, reachable)
			.is_some_and(|error| error.contains("community markup changed"))
	);
}
