use clap::Parser;
use pretty_assertions::assert_eq;

use super::suggestions;
use crate::Args;

#[test]
fn suggests_likely_command_and_subcommand_typos() {
	for (arguments, expected) in [
		(&["a365", "telemtry", "show"][..], "telemetry show"),
		(&["a365", "telemetry", "shwo"][..], "telemetry show"),
		(&["a365", "cach", "prune"][..], "cache prune"),
		(&["a365", "cache", "prne"][..], "cache prune"),
		(&["a365", "doctro"][..], "doctor"),
		(&["a365", "purg"][..], "purge"),
		(&["a365", "sttas"][..], "stats"),
		(&["a365", "udpate"][..], "update"),
	] {
		let args = Args::try_parse_from(arguments.iter().copied()).unwrap();

		assert_eq!(suggestions(&args), vec![expected.to_owned()]);
	}
}

#[test]
fn keeps_unrelated_words_and_forced_queries_as_title_searches() {
	for arguments in [
		&["a365", "telemetry", "this"][..],
		&["a365", "cache", "this"][..],
		&["a365", "show", "telemetry"][..],
		&["a365", "update", "this"][..],
		&["a365", "--query", "telemtry show"][..],
	] {
		let args = Args::try_parse_from(arguments.iter().copied()).unwrap();

		assert_eq!(suggestions(&args), Vec::<String>::new());
	}
}
