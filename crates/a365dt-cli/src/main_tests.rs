use std::{num::NonZeroUsize, sync::Mutex};

use clap::Parser;
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{
	Args, CacheCommand, Commands, ConfigCommand, MediaAction, TelemetryCommand,
	cancel_download,
	command_line::{OwnerRoute, owner_route, route_title_query},
	media_action, opens_tui,
};
use crate::interactive::{self, Anime365Access};

#[test]
fn parses_configuration_commands() {
	let interactive = Args::try_parse_from(["a365", "config"]).unwrap();
	let show = Args::try_parse_from(["a365", "config", "show"]).unwrap();
	let reset =
		Args::try_parse_from(["a365", "config", "reset", "--yes"]).unwrap();

	assert!(matches!(
		interactive.command,
		Some(Commands::Config { command: None })
	));
	assert!(matches!(
		show.command,
		Some(Commands::Config {
			command: Some(ConfigCommand::Show { query })
		}) if query.is_empty()
	));
	assert!(matches!(
		reset.command,
		Some(Commands::Config {
			command: Some(ConfigCommand::Reset { yes: true, query })
		}) if query.is_empty()
	));
}

#[test]
fn routes_interrupts_to_active_downloads() {
	let (cancel, mut cancellation) = watch::channel(false);
	let active_download = Mutex::new(Some(cancel));

	assert_eq!(
		(
			cancel_download(&active_download),
			*cancellation.borrow_and_update(),
		),
		(true, true)
	);
}

#[test]
fn only_interactive_playback_routes_open_the_tui() {
	let opens = |arguments: &[&str]| {
		let mut args = Args::try_parse_from(arguments).unwrap();
		route_title_query(&mut args);
		opens_tui(&args)
	};

	assert_eq!(
		[
			opens(&["a365"]),
			opens(&["a365", "Frieren"]),
			opens(&["a365", "profile"]),
			opens(&["a365", "anilist", "list"]),
			opens(&["a365", "anilist", "status"]),
			opens(&["a365", "update"]),
			opens(&["a365", "Frieren", "--download"]),
		],
		[true, true, true, true, false, false, false],
	);
}

#[test]
fn forces_multi_word_command_names_through_title_search() {
	let args =
		Args::try_parse_from(["a365", "--query", "cache", "prune"]).unwrap();

	assert_eq!(
		(args.forced_query, args.query, args.command.is_none(),),
		(
			vec!["cache".to_owned(), "prune".to_owned()],
			Vec::<String>::new(),
			true
		)
	);
}

#[test]
fn parses_mux_after_query_and_its_aliases() {
	for option in ["--mux", "--burn-subtitles", "--as-single-file"] {
		let args =
			Args::try_parse_from(["a365", "Frieren", "--download", option])
				.unwrap();

		assert_eq!(
			(args.query, args.download, args.mux),
			(vec!["Frieren".to_owned()], true, true)
		);
	}
}

#[test]
fn parses_explicit_download_preference_overrides() {
	let args = Args::try_parse_from([
		"a365",
		"--download",
		"--output",
		"Videos",
		"--jobs",
		"8",
		"Frieren",
	])
	.unwrap();

	assert_eq!(
		(args.output, args.jobs),
		(
			Some(std::path::PathBuf::from("Videos")),
			NonZeroUsize::new(8),
		)
	);
}

#[test]
fn routes_default_and_stream_actions_to_playback() {
	let default = Args::try_parse_from(["a365", "Frieren"]).unwrap();
	let stream = Args::try_parse_from(["a365", "stream", "Frieren"]).unwrap();
	let download =
		Args::try_parse_from(["a365", "Frieren", "--download"]).unwrap();

	assert_eq!(media_action(&default), Ok(MediaAction::Playback));
	assert_eq!(media_action(&stream), Ok(MediaAction::Playback));
	assert_eq!(media_action(&download), Ok(MediaAction::Download));
	assert!(matches!(
		stream.command,
		Some(Commands::Stream { query }) if query == ["Frieren"]
	));
}

#[test]
fn defers_anime365_access_for_anilist_and_timetable_destinations() {
	let anilist_login =
		Args::try_parse_from(["a365", "anilist", "login"]).unwrap();
	let anilist_list =
		Args::try_parse_from(["a365", "anilist", "list"]).unwrap();
	let timetable = Args::try_parse_from(["a365", "timetable"]).unwrap();
	let search = Args::try_parse_from(["a365", "Frieren"]).unwrap();

	assert_eq!(
		[
			interactive::anime365_access(&anilist_login),
			interactive::anime365_access(&anilist_list),
			interactive::anime365_access(&timetable),
			interactive::anime365_access(&search),
		],
		[
			Anime365Access::Deferred,
			Anime365Access::Deferred,
			Anime365Access::Deferred,
			Anime365Access::Required,
		]
	);
}

#[test]
fn routes_explicit_interactive_commands_to_their_tui_destinations() {
	for (arguments, expected) in [
		(&["a365"][..], crate::tui::Destination::Home),
		(&["a365", "Frieren"][..], crate::tui::Destination::Search),
		(
			&["a365", "stream", "Frieren"][..],
			crate::tui::Destination::Search,
		),
		(
			&["a365", "timetable"][..],
			crate::tui::Destination::Timetable,
		),
		(&["a365", "moments"][..], crate::tui::Destination::Moments),
		(&["a365", "profile"][..], crate::tui::Destination::Profile),
		(
			&["a365", "anilist", "list"][..],
			crate::tui::Destination::AniList,
		),
	] {
		let args = Args::try_parse_from(arguments.iter().copied()).unwrap();
		let query = match &args.command {
			Some(Commands::Stream { query }) => query.join(" "),
			_ => args.query.join(" "),
		};

		assert_eq!(interactive::destination(&args, &query), expected);
	}
}

#[test]
fn rejects_download_only_options_during_playback() {
	for arguments in [
		&["a365", "Frieren", "--output", "Videos"][..],
		&["a365", "Frieren", "--jobs", "8"][..],
		&["a365", "Frieren", "--mux"][..],
		&["a365", "--download", "stream", "Frieren"][..],
	] {
		let args = Args::try_parse_from(arguments.iter().copied()).unwrap();
		assert!(media_action(&args).is_err());
	}
}

#[test]
fn parses_telemetry_control_commands() {
	let args = Args::try_parse_from(["a365", "telemetry", "disable"]).unwrap();

	assert!(matches!(
		args.command,
		Some(Commands::Telemetry {
			command: TelemetryCommand::Disable { query }
		})
			if query.is_empty()
	));
}

#[test]
fn parses_guarded_full_and_partial_telemetry_clears() {
	assert_eq!(
		[
			clear_args(&["--since", "30m"]),
			clear_args(&["--since", "30", "minutes"]),
			clear_args(&["--since", "this year"]),
		],
		[
			(false, Some(vec!["30m".into()])),
			(false, Some(vec!["30".into(), "minutes".into()])),
			(false, Some(vec!["this year".into()])),
		]
	);
	for option in ["-y", "--yes"] {
		assert_eq!(clear_args(&[option]), (true, None));
	}
}

#[test]
fn rejects_conflicting_or_oversized_telemetry_clear_options() {
	for arguments in [
		&["--yes", "--since", "30m"][..],
		&["--since", "30", "minutes", "ago"][..],
		&["--since", "30m", "--since", "1h"][..],
	] {
		let arguments = ["a365", "telemetry", "clear"]
			.into_iter()
			.chain(arguments.iter().copied());
		assert!(Args::try_parse_from(arguments).is_err());
	}
}

fn clear_args(arguments: &[&str]) -> (bool, Option<Vec<String>>) {
	let arguments = ["a365", "telemetry", "clear"]
		.into_iter()
		.chain(arguments.iter().copied());
	let args = Args::try_parse_from(arguments).unwrap();
	let Some(Commands::Telemetry {
		command: TelemetryCommand::Clear { yes, since, query },
	}) = args.command
	else {
		panic!("expected telemetry clear");
	};
	assert!(query.is_empty());
	(yes, since)
}

#[test]
fn parses_purge_confirmation_options() {
	for (arguments, expected) in [
		(&["a365", "purge"][..], false),
		(&["a365", "purge", "-y"][..], true),
		(&["a365", "purge", "--yes"][..], true),
	] {
		let args = Args::try_parse_from(arguments.iter().copied()).unwrap();

		assert!(matches!(
			args.command,
			Some(Commands::Purge { yes }) if yes == expected
		));
	}
}

#[test]
fn routes_unknown_command_arguments_through_title_search() {
	for arguments in [
		&["a365", "cache", "this"][..],
		&["a365", "cache", "prune", "this"][..],
		&["a365", "completions", "this"][..],
		&["a365", "completions", "zsh", "this"][..],
		&["a365", "config", "this"][..],
		&["a365", "config", "show", "this"][..],
		&["a365", "doctor", "elise"][..],
		&["a365", "stats", "this"][..],
		&["a365", "telemetry", "this"][..],
		&["a365", "telemetry", "show", "this"][..],
		&["a365", "update", "this"][..],
	] {
		let mut args = Args::try_parse_from(arguments.iter().copied()).unwrap();

		route_title_query(&mut args);

		assert_eq!(
			(args.query, args.command.is_none()),
			(
				arguments[1..].iter().copied().map(str::to_owned).collect(),
				true
			)
		);
	}
}

#[test]
fn preserves_existing_commands() {
	let mut cache = Args::try_parse_from(["a365", "cache", "prune"]).unwrap();
	let mut clear = Args::try_parse_from([
		"a365",
		"telemetry",
		"clear",
		"--since",
		"yesterday",
	])
	.unwrap();
	let mut completions =
		Args::try_parse_from(["a365", "completions", "zsh"]).unwrap();
	let mut doctor = Args::try_parse_from(["a365", "doctor"]).unwrap();
	let mut stats = Args::try_parse_from(["a365", "stats"]).unwrap();
	let mut update = Args::try_parse_from(["a365", "update"]).unwrap();

	route_title_query(&mut cache);
	route_title_query(&mut clear);
	route_title_query(&mut completions);
	route_title_query(&mut doctor);
	route_title_query(&mut stats);
	route_title_query(&mut update);

	assert!(matches!(
		cache.command,
		Some(Commands::Cache {
			command: CacheCommand::Prune { query, .. }
		})
			if query.is_empty()
	));
	assert!(matches!(
		clear.command,
		Some(Commands::Telemetry {
			command: TelemetryCommand::Clear { since: Some(_), .. }
		})
	));
	assert!(matches!(
		completions.command,
		Some(Commands::Completions { arguments })
			if arguments == ["zsh"]
	));
	assert!(matches!(
		doctor.command,
		Some(Commands::Doctor { query }) if query.is_empty()
	));
	assert!(matches!(
		stats.command,
		Some(Commands::Stats { query }) if query.is_empty()
	));
	assert!(matches!(
		update.command,
		Some(Commands::Update { query }) if query.is_empty()
	));
}

#[test]
fn accepts_preauthorized_cache_rebuilds() {
	let args =
		Args::try_parse_from(["a365", "cache", "prune", "--yes"]).unwrap();

	assert!(matches!(
		args.command,
		Some(Commands::Cache {
			command: CacheCommand::Prune {
				yes: true,
				query,
			}
		}) if query.is_empty()
	));
}

#[test]
fn routes_commands_to_only_their_required_owners() {
	for (arguments, expected) in [
		(&["a365", "purge", "--yes"][..], OwnerRoute::Purge),
		(
			&["a365", "telemetry", "show"][..],
			OwnerRoute::TelemetryControl,
		),
		(&["a365", "completions", "zsh"][..], OwnerRoute::Stateless),
		(&["a365", "config"][..], OwnerRoute::PreferencesOnly),
		(&["a365", "config", "show"][..], OwnerRoute::PreferencesOnly),
		(
			&["a365", "cache", "prune", "--yes"][..],
			OwnerRoute::CachePruneAndTelemetry,
		),
		(&["a365", "doctor"][..], OwnerRoute::CacheAndTelemetry),
		(&["a365", "anilist", "status"][..], OwnerRoute::AccountOnly),
		(
			&["a365", "anilist", "login"][..],
			OwnerRoute::CacheAndTelemetry,
		),
		(
			&["a365", "anilist", "list"][..],
			OwnerRoute::CacheAndTelemetry,
		),
		(&["a365", "moments"][..], OwnerRoute::CacheAndTelemetry),
		(&["a365", "profile"][..], OwnerRoute::CacheAndTelemetry),
		(&["a365", "timetable"][..], OwnerRoute::CacheAndTelemetry),
		(&["a365", "stats"][..], OwnerRoute::CacheAndTelemetry),
		(
			&["a365", "stream", "Frieren"][..],
			OwnerRoute::CacheAndTelemetry,
		),
		(&["a365", "update"][..], OwnerRoute::CacheAndTelemetry),
		(&["a365", "Frieren"][..], OwnerRoute::CacheAndTelemetry),
	] {
		let mut args = Args::try_parse_from(arguments.iter().copied()).unwrap();
		route_title_query(&mut args);

		assert_eq!(owner_route(&args), expected);
	}
}
