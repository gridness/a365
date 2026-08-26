use std::{env, process::Command};

fn main() {
	println!("cargo:rerun-if-changed=../../.git/HEAD");
	for variable in [
		"A365_BUILD_PROFILE",
		"A365DT_BUILD_PROFILE",
		"A365_COMMIT_SHA",
		"A365DT_COMMIT_SHA",
		"A365_RUSTC",
		"A365DT_RUSTC",
	] {
		println!("cargo:rerun-if-env-changed={variable}");
	}
	println!(
		"cargo:rustc-env=A365_BUILD_PROFILE={}",
		build_value("A365_BUILD_PROFILE", "A365DT_BUILD_PROFILE")
			.or_else(|| env::var("PROFILE").ok())
			.unwrap_or_else(|| "unknown".into())
	);
	println!(
		"cargo:rustc-env=A365_COMMIT_SHA={}",
		build_value("A365_COMMIT_SHA", "A365DT_COMMIT_SHA")
			.or_else(|| output("git", &["rev-parse", "--short=8", "HEAD"]))
			.unwrap_or_else(|| "unknown".into())
	);
	let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
	println!(
		"cargo:rustc-env=A365_RUSTC={}",
		build_value("A365_RUSTC", "A365DT_RUSTC")
			.or_else(|| output(&rustc, &["--version"]))
			.unwrap_or_else(|| "unknown".into())
	);
}

fn build_value(current: &str, legacy: &str) -> Option<String> {
	env::var(current)
		.ok()
		.or_else(|| env::var(legacy).ok())
		.filter(|value| !value.trim().is_empty())
		.map(|value| value.trim().to_owned())
}

fn output(program: &str, arguments: &[&str]) -> Option<String> {
	let output = Command::new(program).args(arguments).output().ok()?;
	output
		.status
		.success()
		.then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
