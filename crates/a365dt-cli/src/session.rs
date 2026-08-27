use std::{
	io::IsTerminal,
	process::ExitCode,
	sync::{Arc, Mutex},
};

use tokio::{fs, sync::watch};

use super::{
	Args, Commands, MediaAction, fetch_embeds, ffmpeg_available, media_action,
	print_summary, series_recording,
};
use crate::{
	anilist,
	api::Anime365,
	auth, cache,
	download::{self, Job, Status},
	error::Error,
	interactive, poster, preferences, select, series_search, startup,
	telemetry, tui, ui,
};

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalMode {
	Interactive,
	NonInteractive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Presentation {
	QuietTui,
	Detailed,
}

pub(super) async fn run(
	args: Args,
	active_download: Arc<Mutex<Option<watch::Sender<bool>>>>,
	store: &cache::Store,
	telemetry: &telemetry::Recorder,
) -> Result<ExitCode, Error> {
	let action = media_action(&args)?;
	let presentation = presentation(action, args.debug);
	if presentation == Presentation::Detailed {
		ui::heading("a365  ◆  Anime365 player and downloader");
	}
	let terminal =
		if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
			TerminalMode::Interactive
		} else {
			TerminalMode::NonInteractive
		};
	require_interactive_playback(action, terminal)?;
	let preference_store = preferences::Store::discover()?;
	let overrides = preferences::Overrides {
		output: args.output.clone(),
		jobs: args.jobs,
		mux: args.mux,
	};
	let configured_preferences =
		preference_store.load(preferences::Overrides::default())?;
	let preferences = preference_store.load(overrides.clone())?;
	if presentation == Presentation::Detailed {
		preferences::warn_if_high_concurrency(&preferences);
	}
	if let Some(Commands::Anilist {
		command: command @ anilist::Command::Login,
	}) = args.command.as_ref()
	{
		anilist::run(command).await?;
	}
	let notices = startup::load(store).await;
	if presentation == Presentation::Detailed {
		startup::show(&notices);
	}
	let apis = if interactive::anime365_access(&args)
		== interactive::Anime365Access::Required
	{
		authenticate_content_sources(&preferences, telemetry, presentation)
			.await?
	} else {
		Vec::new()
	};

	let query = if let Some(Commands::Stream { query }) = &args.command {
		query.join(" ")
	} else if args.forced_query.is_empty() {
		args.query.join(" ")
	} else {
		args.forced_query.join(" ")
	};
	if action == MediaAction::Playback {
		let continue_watching = crate::continue_watching::Store::discover()?;
		let destination = interactive::destination(&args, &query);
		let (cancel, cancellation) = watch::channel(false);
		*active_download.lock().unwrap() = Some(cancel.clone());
		let result = tui::run(tui::Request {
			apis: &apis,
			store,
			preferences: &preferences,
			configured_preferences: &configured_preferences,
			preference_store: &preference_store,
			overrides,
			telemetry,
			destination,
			query,
			cancelled: cancellation,
			session_cancel: cancel,
			active_playback: Arc::clone(&active_download),
			continue_watching: &continue_watching,
			tip: notices.plain_tip(),
			upgrade: notices.tui_update_notice(),
			debug: args.debug,
		})
		.await;
		*active_download.lock().unwrap() = None;
		return result;
	}
	let selected =
		series_search::choose(&apis, store, query, telemetry).await?;
	let series_recording = series_recording(&selected.series, &preferences);
	telemetry.record_series(
		&selected.series,
		selected.catalogue,
		series_recording,
	);
	let series = selected.series;
	let api = apis
		.iter()
		.find(|api| api.source() == series.source)
		.cloned()
		.ok_or_else(|| {
			Error::new(format!(
				"{} is not enabled for this invocation.",
				series.source
			))
		})?;
	ui::success(format!("Selected {}", series.title));
	poster::show(&api, &series).await;
	let episodes = select::choose_episodes(&series.episodes)?;
	let translations = api.translations(series.id).await?;
	let (track, releases) =
		select::choose_track(translations.clone(), &episodes)?;
	ui::success(format!(
		"Selected {}-{} by {}",
		track.kind, track.language, track.authors
	));

	ui::note("Loading available media…");
	let releases = fetch_embeds(&api, releases, preferences.jobs.get()).await?;
	let planned = select::choose_resolutions(releases)?;
	let separate_subtitles = planned
		.iter()
		.filter(|release| release.subtitle_url.is_some())
		.count();
	let embedded = planned.len() - separate_subtitles;
	if track.kind == "sub" && embedded > 0 {
		ui::note(format!(
			"{embedded} episode(s) have subtitles contained in the MP4."
		));
	}
	let mux = if separate_subtitles > 0 && ffmpeg_available().await {
		preferences.mux
			|| ui::confirm(
				"Mux separate ASS subtitles into MKV after download?",
				false,
			)?
	} else {
		if separate_subtitles > 0 {
			ui::warning(
				"ffmpeg is unavailable; keeping MP4 and ASS files separate.",
			);
		}
		false
	};

	let output = preference_store.prepare_output(&preferences.output)?;
	let directory = output.join(download::sanitize(&series.title, 100));
	fs::create_dir_all(&directory).await.map_err(|error| {
		Error::with_debug(
			format!(
				"Could not create output directory {}.",
				directory.display()
			),
			error,
		)
	})?;
	ui::note(format!("Output: {}", directory.display()));
	let jobs = planned
		.into_iter()
		.map(|release| Job::new(release, directory.clone(), mux))
		.collect();
	let (cancel, cancellation) = watch::channel(false);
	*active_download.lock().unwrap() = Some(cancel);
	let summary = download::run(
		api,
		jobs,
		preferences.jobs.get(),
		args.debug,
		cancellation,
	)
	.await;
	*active_download.lock().unwrap() = None;
	telemetry.record_download(&series, &summary, series_recording);
	print_summary(&summary, &directory, args.debug);
	ui::alert();
	let interrupted = summary
		.outcomes
		.iter()
		.any(|outcome| outcome.status == Status::Interrupted);
	let failed = summary.outcomes.iter().any(|outcome| {
		matches!(
			outcome.status,
			Status::Failed | Status::MuxFailed | Status::Interrupted
		)
	});
	Ok(if interrupted {
		ExitCode::from(130)
	} else if failed {
		ExitCode::FAILURE
	} else {
		ExitCode::SUCCESS
	})
}

async fn authenticate_content_sources(
	preferences: &preferences::Preferences,
	telemetry: &telemetry::Recorder,
	presentation: Presentation,
) -> Result<Vec<Anime365>, Error> {
	let access_token = match presentation {
		Presentation::QuietTui => auth::access_token_silently()?,
		Presentation::Detailed => auth::access_token()?,
	};
	let anime365 =
		Anime365::new(access_token.value().to_owned(), telemetry.clone())?;
	if presentation == Presentation::Detailed {
		ui::note("Validating Anime365 access…");
	}
	anime365.validate().await?;
	let mut apis = vec![anime365];
	if preferences.adult {
		let h365 =
			Anime365::h365(access_token.value().to_owned(), telemetry.clone())?;
		if presentation == Presentation::Detailed {
			ui::note("Validating H365 access…");
		}
		match h365.validate().await {
			Ok(()) => apis.push(h365),
			Err(error) => {
				if presentation == Presentation::Detailed {
					ui::warning(error.context(
						"H365 is unavailable; continuing with Anime365",
					));
				}
			}
		}
	}
	if presentation == Presentation::Detailed {
		ui::success("Authenticated");
	}
	auth::store_if_requested(&access_token)?;
	Ok(apis)
}

const fn presentation(action: MediaAction, debug: bool) -> Presentation {
	if matches!(action, MediaAction::Playback) && !debug {
		Presentation::QuietTui
	} else {
		Presentation::Detailed
	}
}

fn require_interactive_playback(
	action: MediaAction,
	terminal: TerminalMode,
) -> Result<(), Error> {
	if action == MediaAction::Playback
		&& terminal == TerminalMode::NonInteractive
	{
		return Err(Error::new(
			"Playback opens the full-screen TUI and requires an interactive terminal. Use `--download` for a non-interactive download flow.",
		));
	}
	Ok(())
}
