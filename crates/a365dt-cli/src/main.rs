mod anilist;
mod api;
mod app_files;
mod auth;
mod cache;
mod command_line;
mod community;
mod content;
mod doctor;
mod download;
mod episode_playback;
mod error;
mod interactive;
mod playback;
mod poster;
mod preferences;
mod search;
mod select;
mod series_search;
mod session;
mod sqlite;
mod startup;
mod stats;
mod telemetry;
mod tui;
mod ui;

#[cfg(test)]
#[path = "search_tests.rs"]
mod search_tests;

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;

use std::{
	collections::VecDeque,
	num::NonZeroUsize,
	path::PathBuf,
	process::{self, ExitCode},
	sync::{Arc, Mutex},
};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::aot::{Shell, generate};
use console::style;
use indicatif::{HumanBytes, HumanDuration};
use tokio::{process::Command, signal, sync::watch, task::JoinSet};

use crate::{
	api::{Anime365, Episode, Translation},
	command_line::OwnerRoute,
	content::ContentSource,
	download::Status,
	error::Error,
	preferences::ConfigCommand,
	select::Release,
};

#[derive(Parser)]
#[command(
	name = "a365",
	version,
	about = "Find, play, and download Anime365 and H365 episodes"
)]
struct Args {
	#[command(subcommand)]
	command: Option<Commands>,

	#[arg(value_name = "QUERY_OR_URL", num_args = 0..)]
	query: Vec<String>,

	/// Search for a title even when it matches a command name.
	#[arg(
		long = "query",
		value_name = "QUERY",
		num_args = 1..,
		conflicts_with = "query"
	)]
	forced_query: Vec<String>,

	/// Override the configured output directory for this Invocation.
	#[arg(short, long, value_name = "DIR")]
	output: Option<PathBuf>,

	/// Override the configured number of concurrent downloads for this Invocation.
	#[arg(short, long)]
	jobs: Option<NonZeroUsize>,

	/// Download selected Episodes instead of playing them.
	#[arg(long)]
	download: bool,

	/// Mux separate ASS subtitles into MKV without confirmation.
	#[arg(
		long,
		visible_aliases = ["burn-subtitles", "as-single-file"]
	)]
	mux: bool,

	/// Show technical error details.
	#[arg(long, global = true)]
	debug: bool,
}

#[derive(Subcommand)]
enum Commands {
	/// Connect and inspect a read-only AniList account.
	Anilist {
		#[command(subcommand)]
		command: anilist::Command,
	},

	/// Manage the local cache.
	Cache {
		#[command(subcommand)]
		command: CacheCommand,
	},

	/// Generate shell completions.
	Completions {
		#[arg(value_name = "SHELL", num_args = 1..)]
		arguments: Vec<String>,
	},

	/// Configure persistent preferences.
	Config {
		#[command(subcommand)]
		command: Option<ConfigCommand>,
	},

	/// Check a365, Anime365, preferences, cache, and telemetry health.
	Doctor {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	/// Permanently remove all local a365 application data.
	Purge {
		/// Purge without asking for confirmation.
		#[arg(short, long)]
		yes: bool,
	},

	/// Browse and play public Anime365 Moments.
	Moments,

	/// Show the connected Anime365 profile and public lists.
	Profile,

	/// Show local cache, usage, and performance statistics.
	Stats {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	/// Explicitly open the default Playback flow.
	Stream {
		#[arg(value_name = "QUERY_OR_URL", num_args = 0..)]
		query: Vec<String>,
	},

	/// Inspect or control local usage telemetry.
	Telemetry {
		#[command(subcommand)]
		command: TelemetryCommand,
	},

	/// Show the current local week from AniList's airing schedule.
	Timetable,

	/// Check whether a newer stable a365 release is available.
	Update {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},
}

#[derive(Subcommand)]
enum CacheCommand {
	/// Clear the local cache.
	Prune {
		/// Rebuild damaged cache storage without confirmation.
		#[arg(short, long)]
		yes: bool,

		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	#[command(external_subcommand)]
	Query(Vec<String>),
}

#[derive(Subcommand)]
enum TelemetryCommand {
	/// Clear collected telemetry without changing collection state.
	Clear {
		/// Clear all telemetry without asking for confirmation.
		#[arg(short, long, conflicts_with = "since")]
		yes: bool,

		/// Clear telemetry since 30m, 30 minutes, today, this week, this month,
		/// or this year.
		#[arg(
			long,
			value_name = "EXPRESSION",
			num_args = 1..=2,
			action = clap::ArgAction::Set,
			conflicts_with = "query"
		)]
		since: Option<Vec<String>>,

		#[arg(
			value_name = "QUERY",
			num_args = 0..,
			hide = true,
			conflicts_with = "yes"
		)]
		query: Vec<String>,
	},

	/// Stop collecting local telemetry.
	Disable {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	/// Resume collecting local telemetry.
	Enable {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	/// Show every collected field and its current value.
	Show {
		#[arg(value_name = "QUERY", num_args = 0.., hide = true)]
		query: Vec<String>,
	},

	#[command(external_subcommand)]
	Query(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaAction {
	Download,
	Playback,
}

#[tokio::main]
async fn main() -> ExitCode {
	let mut args = Args::parse();
	let invocation_id = telemetry::InvocationId::new();
	ui::init();
	let debug = args.debug;
	if !args.forced_query.is_empty() && args.command.is_some() {
		ui::failure(
			"`--query` cannot be combined with a command. Remove the command or search terms.",
		);
		return ExitCode::FAILURE;
	}
	let suggestions = command_line::suggestions(&args);
	if !suggestions.is_empty() {
		ui::failure(command_line::suggestion_message(&suggestions));
		return ExitCode::FAILURE;
	}
	command_line::route_title_query(&mut args);
	if let Some(Commands::Update { .. }) = args.command.as_ref() {
		println!("a365 {}\n", env!("CARGO_PKG_VERSION"));
	}
	let owner_route = command_line::owner_route(&args);
	if owner_route == OwnerRoute::Purge {
		let Some(Commands::Purge { yes }) = args.command.as_ref() else {
			unreachable!("the purge route contains a purge command")
		};
		let confirmed = if *yes {
			true
		} else {
			match ui::confirm(
				&ui::red(
					"Permanently remove all local a365 application data and saved credentials?",
				),
				false,
			) {
				Ok(confirmed) => confirmed,
				Err(error) => {
					ui::failure(error.render(debug));
					return ExitCode::FAILURE;
				}
			}
		};
		if !confirmed {
			ui::note("Purge cancelled.");
			return ExitCode::SUCCESS;
		}
		let files = app_files::purge().map_err(|error| {
			Error::with_debug(
				"Could not remove all local a365 application files.",
				error,
			)
		});
		let anime365_token = auth::remove_stored_token();
		let anilist_token = anilist::remove_stored_token();
		return match files.and(anime365_token).and(anilist_token) {
			Ok(()) => {
				ui::success("Local a365 application data removed");
				ExitCode::SUCCESS
			}
			Err(error) => {
				ui::failure(error.render(debug));
				ExitCode::FAILURE
			}
		};
	}
	if owner_route == OwnerRoute::Stateless {
		let Some(Commands::Completions { arguments }) = args.command.as_ref()
		else {
			unreachable!("the stateless route generates completions")
		};
		generate(
			completion_shell(arguments)
				.expect("invalid completion shells return to title search"),
			&mut Args::command(),
			"a365",
			&mut std::io::stdout(),
		);
		return ExitCode::SUCCESS;
	}
	if let Err(error) = app_files::prepare_for_command().await {
		ui::failure(error.render(debug));
		return ExitCode::FAILURE;
	}
	if owner_route == OwnerRoute::PreferencesOnly {
		let Some(Commands::Config { command }) = args.command.as_ref() else {
			unreachable!("the preferences route contains a config command")
		};
		let result = match preferences::Store::discover() {
			Ok(store) => match command {
				None => store.configure().await,
				Some(ConfigCommand::Show { .. }) => store.show(),
				Some(ConfigCommand::Reset { yes, .. }) => {
					store.reset_command(if *yes {
						preferences::ResetPermission::Preauthorized
					} else {
						preferences::ResetPermission::Ask
					})
				}
				Some(ConfigCommand::Query(_)) => {
					unreachable!("config queries return to title search")
				}
			},
			Err(error) => Err(error),
		};
		return match result {
			Ok(()) => ExitCode::SUCCESS,
			Err(error) => {
				ui::failure(error.render(debug));
				ExitCode::FAILURE
			}
		};
	}
	if owner_route == OwnerRoute::AccountOnly {
		let result = match args.command.as_ref() {
			Some(Commands::Anilist { command }) => anilist::run(command).await,
			_ => unreachable!("the account route contains an account command"),
		};
		return match result {
			Ok(()) => ExitCode::SUCCESS,
			Err(error) => {
				ui::failure(error.render(debug));
				ExitCode::FAILURE
			}
		};
	}
	if owner_route == OwnerRoute::TelemetryControl {
		let Some(Commands::Telemetry { command }) = args.command.as_ref()
		else {
			unreachable!("the Telemetry control route contains its command")
		};
		return match run_telemetry(command, invocation_id).await {
			Ok(()) => ExitCode::SUCCESS,
			Err(error) => {
				ui::failure(error.render(debug));
				ExitCode::FAILURE
			}
		};
	}
	let command = telemetry_command(&args);
	let (telemetry, telemetry_writer) =
		telemetry::Writer::open(invocation_id).await;
	if let Some(error) = telemetry_writer.initialization_warning() {
		ui::warning(error.render(debug));
	}
	let active_download = Arc::new(Mutex::new(None));
	let interrupt_download = Arc::clone(&active_download);
	drop(tokio::spawn(async move {
		match signal::ctrl_c().await {
			Ok(()) => {
				eprintln!();
				ui::failure("Cancelled.");
				if !cancel_download(&interrupt_download) {
					process::exit(130);
				}
			}
			Err(error) => {
				ui::failure(
					Error::with_debug("Could not listen for Ctrl+C.", error)
						.render(debug),
				);
				process::exit(1);
			}
		}
	}));
	let result = match owner_route {
		OwnerRoute::CachePruneAndTelemetry => {
			let Some(Commands::Cache {
				command: CacheCommand::Prune { yes, .. },
			}) = args.command.as_ref()
			else {
				unreachable!("the cache-prune route contains its command")
			};
			prune_cache(if *yes {
				cache::RebuildPermission::Preauthorized
			} else {
				cache::RebuildPermission::Ask
			})
			.await
		}
		OwnerRoute::CacheAndTelemetry => {
			let store = cache::Store::open().await;
			if let Some(error) = store.initialization_warning() {
				ui::warning(error);
			}
			let result = if let Some(Commands::Doctor { .. }) =
				args.command.as_ref()
			{
				Ok(doctor::run(&store, &telemetry_writer, debug).await)
			} else if let Some(Commands::Stats { .. }) = args.command.as_ref() {
				stats::run(&store, &telemetry_writer).await;
				Ok(ExitCode::SUCCESS)
			} else if let Some(Commands::Update { .. }) = args.command.as_ref()
			{
				startup::check(&store).await.map(|update| {
					if let Some(update) = update {
						startup::show_update(&update);
					} else {
						ui::success("Already up to date");
					}
					ExitCode::SUCCESS
				})
			} else {
				session::run(args, active_download, &store, &telemetry).await
			};
			store.close().await;
			result
		}
		OwnerRoute::Purge
		| OwnerRoute::Stateless
		| OwnerRoute::PreferencesOnly
		| OwnerRoute::AccountOnly
		| OwnerRoute::TelemetryControl => {
			unreachable!("early-return routes do not open ordinary owners")
		}
	};
	let (code, outcome) = match result {
		Ok(code) if code == ExitCode::SUCCESS => {
			(code, telemetry::CommandOutcome::Success)
		}
		Ok(code) if code == ExitCode::from(130) => {
			(code, telemetry::CommandOutcome::Cancelled)
		}
		Ok(code) => (code, telemetry::CommandOutcome::Failure),
		Err(error) => {
			let outcome = if error.message() == "Cancelled." {
				telemetry::CommandOutcome::Cancelled
			} else {
				telemetry::CommandOutcome::Failure
			};
			ui::failure(error.render(debug));
			(ExitCode::FAILURE, outcome)
		}
	};
	telemetry.record_command(command, outcome);
	if let Err(error) = telemetry_writer.finish().await {
		ui::warning(error.render(debug));
	}
	code
}

fn completion_shell(arguments: &[String]) -> Option<Shell> {
	let [shell] = arguments else {
		return None;
	};
	shell.parse().ok()
}

async fn run_telemetry(
	command: &TelemetryCommand,
	invocation_id: telemetry::InvocationId,
) -> Result<(), Error> {
	match command {
		TelemetryCommand::Clear { yes, since, .. } => {
			let request = match (*yes, since) {
				(true, None) => telemetry::ClearRequest::All(
					telemetry::FullClearPermission::Preauthorized,
				),
				(false, None) => telemetry::ClearRequest::All(
					telemetry::FullClearPermission::Ask,
				),
				(false, Some(since)) => {
					telemetry::ClearRequest::Since(since.clone())
				}
				(true, Some(_)) => {
					unreachable!("clap rejects --yes with --since")
				}
			};
			telemetry::clear(request).await
		}
		TelemetryCommand::Disable { .. } => {
			telemetry::disable(invocation_id).await
		}
		TelemetryCommand::Enable { .. } => {
			telemetry::enable(invocation_id).await
		}
		TelemetryCommand::Show { .. } => telemetry::show(invocation_id).await,
		TelemetryCommand::Query(_) => {
			unreachable!("telemetry queries return to title search")
		}
	}
}

fn telemetry_command(args: &Args) -> telemetry::Command {
	match &args.command {
		Some(Commands::Cache {
			command: CacheCommand::Prune { .. },
		}) => telemetry::Command::CachePrune,
		Some(Commands::Cache {
			command: CacheCommand::Query(_),
		}) => unreachable!("cache queries return to title search"),
		Some(Commands::Completions { .. }) => {
			unreachable!("completion generation returns before telemetry")
		}
		Some(Commands::Config { .. }) => {
			unreachable!("configuration commands return before telemetry")
		}
		Some(Commands::Doctor { .. }) => telemetry::Command::Doctor,
		Some(Commands::Anilist { command }) if !command.opens_tui() => {
			unreachable!("account commands return before recording")
		}
		Some(Commands::Anilist { .. })
		| Some(Commands::Moments)
		| Some(Commands::Profile)
		| Some(Commands::Timetable) => telemetry::Command::Playback,
		Some(Commands::Purge { .. }) => {
			unreachable!("purge returns before recording")
		}
		Some(Commands::Stats { .. }) => telemetry::Command::Stats,
		Some(Commands::Stream { .. }) => telemetry::Command::Playback,
		Some(Commands::Telemetry { .. }) => {
			unreachable!("telemetry commands return before recording")
		}
		Some(Commands::Update { .. }) => telemetry::Command::Update,
		None if args.download => telemetry::Command::Download,
		None => telemetry::Command::Playback,
	}
}

fn cancel_download(
	active_download: &Mutex<Option<watch::Sender<bool>>>,
) -> bool {
	active_download
		.lock()
		.unwrap()
		.as_ref()
		.is_some_and(|cancel| cancel.send(true).is_ok())
}

fn media_action(args: &Args) -> Result<MediaAction, Error> {
	if matches!(args.command, Some(Commands::Stream { .. })) && args.download {
		return Err("`stream` and `--download` cannot be combined.".into());
	}
	let action = if args.download {
		MediaAction::Download
	} else {
		MediaAction::Playback
	};
	if action == MediaAction::Playback
		&& (args.output.is_some() || args.jobs.is_some() || args.mux)
	{
		return Err(
			"`--output`, `--jobs`, and `--mux` are download-only; add `--download`."
				.into(),
		);
	}
	Ok(action)
}

async fn prune_cache(
	permission: cache::RebuildPermission,
) -> Result<ExitCode, Error> {
	ui::heading("a365  ◆  Anime365 player and downloader");
	cache::prune(permission).await?;
	ui::success("Local cache cleared");
	Ok(ExitCode::SUCCESS)
}

fn series_recording(
	series: &api::Series,
	preferences: &preferences::Preferences,
) -> telemetry::SeriesRecording {
	if series.source == ContentSource::H365 && !preferences.adult_telemetry {
		telemetry::SeriesRecording::AggregateOnly
	} else {
		telemetry::SeriesRecording::IncludeIdentity
	}
}

async fn fetch_embeds(
	api: &Anime365,
	releases: Vec<(Episode, Translation)>,
	concurrency: usize,
) -> Result<Vec<Release>, Error> {
	let mut pending = VecDeque::from(releases);
	let mut active = JoinSet::new();
	for _ in 0..concurrency {
		spawn_embed(&mut active, &mut pending, api);
	}
	let mut result = Vec::new();
	while let Some(joined) = active.join_next().await {
		result.push(joined.map_err(|error| {
			Error::with_debug(
				"An internal task stopped while loading episode media.",
				error,
			)
		})??);
		spawn_embed(&mut active, &mut pending, api);
	}
	result.sort_by(|left, right| {
		let number = |episode: &Episode| {
			episode.episode_int.parse::<f64>().unwrap_or(f64::MAX)
		};
		number(&left.episode).total_cmp(&number(&right.episode))
	});
	Ok(result)
}

fn spawn_embed(
	active: &mut JoinSet<Result<Release, Error>>,
	pending: &mut VecDeque<(Episode, Translation)>,
	api: &Anime365,
) {
	if let Some((episode, translation)) = pending.pop_front() {
		let api = api.clone();
		active.spawn(async move {
			let embed = api.embed(translation.id).await?;
			Ok(Release {
				episode,
				translation,
				embed,
			})
		});
	}
}

async fn ffmpeg_available() -> bool {
	Command::new("ffmpeg")
		.arg("-version")
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.status()
		.await
		.is_ok_and(|status| status.success())
}

fn print_summary(
	summary: &download::Summary,
	directory: &std::path::Path,
	debug: bool,
) {
	let count = |status| {
		summary
			.outcomes
			.iter()
			.filter(|outcome| outcome.status == status)
			.count()
	};
	let bytes = summary.outcomes.iter().map(|outcome| outcome.bytes).sum();
	ui::heading("Batch summary");
	ui::grid(&[
		[
			style("Downloaded").green().bold().to_string(),
			count(Status::Downloaded).to_string(),
		],
		[
			style("Skipped").cyan().bold().to_string(),
			count(Status::Skipped).to_string(),
		],
		[
			style("Failed").red().bold().to_string(),
			(count(Status::Failed) + count(Status::MuxFailed)).to_string(),
		],
		[
			style("Interrupted").yellow().bold().to_string(),
			count(Status::Interrupted).to_string(),
		],
		[
			style("Size").bold().to_string(),
			HumanBytes(bytes).to_string(),
		],
		[
			style("Elapsed").bold().to_string(),
			HumanDuration(summary.elapsed).to_string(),
		],
		[
			style("Output").bold().to_string(),
			directory.display().to_string(),
		],
	]);
	for outcome in summary.outcomes.iter().filter(|outcome| {
		matches!(
			outcome.status,
			Status::Failed | Status::MuxFailed | Status::Interrupted
		)
	}) {
		ui::failure(format!(
			"{}: {}",
			outcome.episode,
			outcome.detail.render(debug)
		));
	}
	if summary
		.outcomes
		.iter()
		.any(|outcome| outcome.status == Status::Failed)
	{
		ui::note("Run the same command again to resume preserved .part files.");
	}
}
