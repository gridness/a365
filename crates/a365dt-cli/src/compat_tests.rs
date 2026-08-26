use std::ffi::OsString;

use pretty_assertions::assert_eq;

use super::allows_deprecation_notice;

#[test]
fn deprecation_notice_avoids_machine_readable_commands() {
	let arguments =
		|values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();

	assert_eq!(
		[
			allows_deprecation_notice(&arguments(&["--version"])),
			allows_deprecation_notice(&arguments(&["-V"])),
			allows_deprecation_notice(&arguments(&["completions", "zsh"])),
			allows_deprecation_notice(&arguments(&["stream", "Frieren"])),
		],
		[false, false, false, true],
	);
}
