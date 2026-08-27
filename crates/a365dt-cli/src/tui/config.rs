use crate::{
	api::{AccessFailure, Anime365},
	content::ContentSource,
	error::Error,
	preferences::{Overrides, Preferences, Store},
};

use super::state::ConfigChange;

pub(super) struct Applied {
	pub configured: Preferences,
	pub effective: Preferences,
	pub apis: Vec<Anime365>,
	pub content_changed: bool,
	pub message: String,
}

pub(super) async fn apply(
	store: Store,
	overrides: Overrides,
	mut configured: Preferences,
	mut apis: Vec<Anime365>,
	change: ConfigChange,
) -> Result<Applied, Error> {
	let previous_adult = configured.adult;
	match change {
		ConfigChange::Output(output) => {
			configured.output = store.prepare_configured_output(&output)?;
		}
		ConfigChange::Jobs(jobs) => configured.jobs = jobs,
		ConfigChange::Mux(mux) => configured.mux = mux,
		ConfigChange::Adult(adult) => configured.adult = adult,
		ConfigChange::AdultTelemetry(adult_telemetry) => {
			configured.adult_telemetry = adult_telemetry;
		}
		ConfigChange::AutoPlayNextEpisode(auto_play) => {
			configured.auto_play_next_episode = auto_play;
		}
	}

	let warning = if configured.adult && !previous_adult {
		enable_adult_source(&mut apis).await?
	} else {
		if !configured.adult {
			apis.retain(|api| api.source() != ContentSource::H365);
		}
		None
	};
	store.save(&configured)?;
	let effective = store.load(overrides)?;
	let overrides_active = effective != configured;
	let message = match (warning, overrides_active) {
		(Some(warning), true) => {
			format!("Saved · {warning} · command-line overrides remain active")
		}
		(Some(warning), false) => format!("Saved · {warning}"),
		(None, true) => "Saved · command-line overrides remain active".into(),
		(None, false) => "Saved · changes are active now".into(),
	};
	Ok(Applied {
		content_changed: previous_adult != configured.adult,
		configured,
		effective,
		apis,
		message,
	})
}

async fn enable_adult_source(
	apis: &mut Vec<Anime365>,
) -> Result<Option<String>, Error> {
	if apis.iter().any(|api| api.source() == ContentSource::H365) {
		return Ok(None);
	}
	let anime365 = apis
		.iter()
		.find(|api| api.source() == ContentSource::Anime365)
		.ok_or_else(|| {
			Error::new("Adult content needs an authenticated Anime365 session.")
		})?;
	let h365 = anime365.with_source(ContentSource::H365);
	match h365.validate_access().await {
		Ok(()) => {
			apis.push(h365);
			Ok(None)
		}
		Err(AccessFailure::Denied(error)) => {
			Err(error.context("Adult content was not enabled"))
		}
		Err(AccessFailure::Unavailable(error)) => Ok(Some(format!(
			"H365 is temporarily unavailable ({})",
			error.message()
		))),
	}
}
