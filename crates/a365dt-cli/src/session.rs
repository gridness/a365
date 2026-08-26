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
	episode_playback,
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

pub(super) async fn run(
	args: Args,
	active_download: Arc<Mutex<Option<watch::Sender<bool>>>>,
	store: &cache::Store,
	telemetry: &telemetry::Recorder,
) -> Result<ExitCode, Error> {
	ui::heading("a365  ◆  Anime365 player and downloader");
	let action = media_action(&args)?;
	let terminal =
		if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
			TerminalMode::Interactive
		} else {
			TerminalMode::NonInteractive
		};
	require_interactive_playback(action, terminal)?;
	let preference_store = preferences::Store::discover()?;
	let preferences = preference_store.load(preferences::Overrides {
		output: args.output.clone(),
		jobs: args.jobs,
		mux: args.mux,
	})?;
	preferences::warn_if_high_concurrency(&preferences);
	if let Some(Commands::Anilist {
		command: command @ anilist::Command::Login,
	}) = args.command.as_ref()
	{
		anilist::run(command).await?;
	}
	startup::show(store).await;
	let mut apis = if interactive::anime365_access(&args)
		== interactive::Anime365Access::Required
	{
		authenticate_content_sources(&preferences, telemetry).await?
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
	let selected = if action == MediaAction::Playback {
		let destination = interactive::destination(&args, &query);
		let (cancel, cancellation) = watch::channel(false);
		*active_download.lock().unwrap() = Some(cancel);
		let launch = tui::run(
			&apis,
			store,
			&preferences,
			destination,
			query,
			cancellation,
		)
		.await;
		*active_download.lock().unwrap() = None;
		let Some(launch) = launch? else {
			return Ok(ExitCode::SUCCESS);
		};
		if let tui::Launch::Moment { id, title } = launch {
			return interactive::play_moment(id, &title, active_download).await;
		}
		if launch.needs_content_sources() && apis.is_empty() {
			apis =
				authenticate_content_sources(&preferences, telemetry).await?;
		}
		interactive::selection(launch, &apis, store).await?
	} else {
		series_search::choose(&apis, store, query, telemetry).await?
	};
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
	let episodes = match action {
		MediaAction::Download => select::choose_episodes(&series.episodes)?,
		MediaAction::Playback => {
			vec![select::choose_episode(&series.episodes)?]
		}
	};
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
	if action == MediaAction::Playback {
		let Some(first) = planned.into_iter().next() else {
			return Err("No playable Episode was selected.".into());
		};
		return episode_playback::run(episode_playback::Request {
			api,
			series: &series,
			translations: &translations,
			track: &track,
			first,
			continuation: if preferences.auto_play_next_episode {
				episode_playback::Continuation::Enabled
			} else {
				episode_playback::Continuation::Disabled
			},
			telemetry,
			series_recording,
			active_playback: active_download,
		})
		.await;
	}
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
) -> Result<Vec<Anime365>, Error> {
	let access_token = auth::access_token()?;
	let anime365 =
		Anime365::new(access_token.value().to_owned(), telemetry.clone())?;
	ui::note("Validating Anime365 access…");
	anime365.validate().await?;
	let mut apis = vec![anime365];
	if preferences.adult {
		let h365 =
			Anime365::h365(access_token.value().to_owned(), telemetry.clone())?;
		ui::note("Validating H365 access…");
		match h365.validate().await {
			Ok(()) => apis.push(h365),
			Err(error) => ui::warning(
				error.context("H365 is unavailable; continuing with Anime365"),
			),
		}
	}
	ui::success("Authenticated");
	auth::store_if_requested(&access_token)?;
	Ok(apis)
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
