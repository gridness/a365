use clap::{Command, CommandFactory};
use rapidfuzz::distance::osa;

use super::{
	Args, CacheCommand, Commands, ConfigCommand, TelemetryCommand,
	completion_shell,
};
use crate::search::typo_budget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerRoute {
	Purge,
	Stateless,
	PreferencesOnly,
	AccountOnly,
	TelemetryControl,
	CachePruneAndTelemetry,
	CacheAndTelemetry,
}

pub fn owner_route(args: &Args) -> OwnerRoute {
	match &args.command {
		Some(Commands::Purge { .. }) => OwnerRoute::Purge,
		Some(Commands::Telemetry { .. }) => OwnerRoute::TelemetryControl,
		Some(Commands::Completions { .. }) => OwnerRoute::Stateless,
		Some(Commands::Config { .. }) => OwnerRoute::PreferencesOnly,
		Some(Commands::Anilist { command }) if !command.opens_tui() => {
			OwnerRoute::AccountOnly
		}
		Some(Commands::Cache {
			command: CacheCommand::Prune { .. },
		}) => OwnerRoute::CachePruneAndTelemetry,
		Some(Commands::Cache {
			command: CacheCommand::Query(_),
		})
		| Some(Commands::Doctor { .. })
		| Some(Commands::Anilist { .. })
		| Some(Commands::Moments)
		| Some(Commands::Profile)
		| Some(Commands::Stats { .. })
		| Some(Commands::Stream { .. })
		| Some(Commands::Timetable)
		| Some(Commands::Update { .. })
		| None => OwnerRoute::CacheAndTelemetry,
	}
}

pub fn route_title_query(args: &mut Args) {
	if let Some(query) = title_query(args) {
		args.query = query;
		args.command = None;
	}
}

pub fn suggestions(args: &Args) -> Vec<String> {
	if !args.forced_query.is_empty() {
		return Vec::new();
	}
	let query = match &args.command {
		None => args.query.clone(),
		Some(_) => match title_query(args) {
			Some(query) => query,
			None => return Vec::new(),
		},
	};
	let mut suggestions = command_paths()
		.into_iter()
		.filter_map(|command| {
			command_distance(&query, &command)
				.map(|distance| (distance, command))
		})
		.collect::<Vec<_>>();
	suggestions.sort_by_key(|(distance, _)| *distance);
	suggestions
		.into_iter()
		.take(5)
		.map(|(_, command)| command)
		.collect()
}

pub fn suggestion_message(suggestions: &[String]) -> String {
	let mut message =
		"Unknown command or subcommand.\nPerhaps you meant:".to_owned();
	for suggestion in suggestions {
		message.push_str("\n  a365 ");
		message.push_str(suggestion);
	}
	message
		.push_str("\n\nUse `--query` to search for the entered words instead.");
	message
}

fn title_query(args: &Args) -> Option<Vec<String>> {
	let (command, query) = match &args.command {
		Some(Commands::Cache {
			command: CacheCommand::Prune { query, .. },
		}) if !query.is_empty() => (
			"cache",
			std::iter::once("prune".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Cache {
			command: CacheCommand::Query(query),
		}) => ("cache", query.clone()),
		Some(Commands::Completions { arguments })
			if completion_shell(arguments).is_none() =>
		{
			("completions", arguments.clone())
		}
		Some(Commands::Config {
			command: Some(ConfigCommand::Reset { yes: false, query }),
		}) if !query.is_empty() => (
			"config",
			std::iter::once("reset".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Config {
			command: Some(ConfigCommand::Show { query }),
		}) if !query.is_empty() => (
			"config",
			std::iter::once("show".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Config {
			command: Some(ConfigCommand::Query(query)),
		}) => ("config", query.clone()),
		Some(Commands::Doctor { query }) if !query.is_empty() => {
			("doctor", query.clone())
		}
		Some(Commands::Stats { query }) if !query.is_empty() => {
			("stats", query.clone())
		}
		Some(Commands::Telemetry {
			command:
				TelemetryCommand::Clear {
					yes: false,
					since: None,
					query,
				},
		}) if !query.is_empty() => (
			"telemetry",
			std::iter::once("clear".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Telemetry {
			command: TelemetryCommand::Disable { query },
		}) if !query.is_empty() => (
			"telemetry",
			std::iter::once("disable".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Telemetry {
			command: TelemetryCommand::Enable { query },
		}) if !query.is_empty() => (
			"telemetry",
			std::iter::once("enable".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Telemetry {
			command: TelemetryCommand::Show { query },
		}) if !query.is_empty() => (
			"telemetry",
			std::iter::once("show".to_owned())
				.chain(query.clone())
				.collect(),
		),
		Some(Commands::Telemetry {
			command: TelemetryCommand::Query(query),
		}) => ("telemetry", query.clone()),
		Some(Commands::Update { query }) if !query.is_empty() => {
			("update", query.clone())
		}
		_ => return None,
	};
	Some(std::iter::once(command.to_owned()).chain(query).collect())
}

fn command_paths() -> Vec<String> {
	let mut paths = Vec::new();
	collect_paths(&Args::command(), &mut Vec::new(), &mut paths);
	paths
}

fn collect_paths(
	command: &Command,
	prefix: &mut Vec<String>,
	paths: &mut Vec<String>,
) {
	let mut children = command
		.get_subcommands()
		.filter(|command| !command.is_hide_set())
		.peekable();
	if children.peek().is_none() {
		if !prefix.is_empty() {
			paths.push(prefix.join(" "));
		}
		return;
	}
	for child in children {
		prefix.push(child.get_name().to_owned());
		collect_paths(child, prefix, paths);
		prefix.pop();
	}
}

fn command_distance(query: &[String], command: &str) -> Option<usize> {
	let mut distance = 0;
	for (query, command) in query.iter().zip(command.split_whitespace()) {
		let current =
			osa::distance(query.to_ascii_lowercase().chars(), command.chars());
		if current > typo_budget(query.chars().count()) {
			return None;
		}
		distance += current;
	}
	(distance > 0).then_some(distance)
}

#[cfg(test)]
#[path = "command_line_tests.rs"]
mod tests;
