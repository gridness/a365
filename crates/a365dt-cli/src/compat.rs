use std::{
	env,
	ffi::OsString,
	io::IsTerminal,
	path::PathBuf,
	process::{Command, ExitCode},
};

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;

fn main() -> ExitCode {
	let arguments = env::args_os().skip(1).collect::<Vec<_>>();
	if std::io::stderr().is_terminal() && allows_deprecation_notice(&arguments)
	{
		eprintln!(
			"a365dt has been renamed to a365; this compatibility command will be removed in v4."
		);
	}
	let executable = env::current_exe().ok().and_then(|path| {
		path.parent().map(|parent| {
			let mut target = PathBuf::from(parent);
			target.push(if cfg!(windows) { "a365.exe" } else { "a365" });
			target
		})
	});
	let Some(executable) = executable else {
		eprintln!("Could not locate the a365 executable beside a365dt.");
		return ExitCode::FAILURE;
	};
	match Command::new(executable).args(arguments).status() {
		Ok(status) => status
			.code()
			.and_then(|code| u8::try_from(code).ok())
			.map_or(ExitCode::FAILURE, ExitCode::from),
		Err(error) => {
			eprintln!("Could not start a365: {error}");
			ExitCode::FAILURE
		}
	}
}

fn allows_deprecation_notice(arguments: &[OsString]) -> bool {
	!arguments.first().is_some_and(|argument| {
		argument == "--version" || argument == "-V" || argument == "completions"
	})
}
