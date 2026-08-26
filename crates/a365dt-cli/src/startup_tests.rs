use std::path::{Path, PathBuf};

use console::strip_ansi_codes;
use pretty_assertions::assert_eq;
use semver::Version;

use super::{
	InstallationChannel, Update, installation_channel_from_path,
	render_markdown, update_from,
};
use crate::cache::Release;

#[test]
fn renders_supported_inline_markdown_and_visible_links() {
	let rendered = render_markdown(
		"Use **bold**, *italic*, `code`, and [docs](https://example.com/).",
	);

	assert_eq!(
		strip_ansi_codes(&rendered),
		"Use bold, italic, code, and docs (https://example.com/)."
	);
}

#[test]
fn finds_only_strictly_newer_stable_releases() {
	let release = |tag_name: &str| Release {
		tag_name: tag_name.to_owned(),
		html_url: "https://example.com/release".to_owned(),
	};

	assert_eq!(
		[
			update_from("0.6.3", release("v0.7.0")),
			update_from("0.6.3", release("v0.6.3")),
			update_from("0.6.3", release("v0.6.2")),
			update_from("0.6.3", release("v0.7.0-beta.1")),
		],
		[
			Ok(Some(Update {
				installed: Version::new(0, 6, 3),
				available: Version::new(0, 7, 0),
				release_url: "https://example.com/release".to_owned(),
			})),
			Ok(None),
			Ok(None),
			Ok(None),
		]
	);
}

#[test]
fn rejects_versions_that_cannot_be_compared() {
	let release = |tag_name: &str| Release {
		tag_name: tag_name.to_owned(),
		html_url: "https://example.com/release".to_owned(),
	};

	assert_eq!(
		[
			update_from("development", release("v0.7.0")),
			update_from("0.6.3", release("latest")),
		]
		.map(|result| result.map_err(|error| error.message().to_owned())),
		[
			Err("Could not check for updates.".to_owned()),
			Err("Could not check for updates.".to_owned()),
		]
	);
}

#[test]
fn infers_managed_installation_channels_from_paths() {
	let cargo_bins = [PathBuf::from("/Users/me/.cargo/bin")];
	let channel =
		|path| installation_channel_from_path(Path::new(path), &cargo_bins);

	assert_eq!(
		[
			channel("/opt/homebrew/Cellar/a365/0.7.0/bin/a365"),
			channel("/Users/me/.cargo/bin/a365"),
			channel("/usr/local/bin/a365"),
		],
		[
			InstallationChannel::Homebrew,
			InstallationChannel::Cargo,
			InstallationChannel::Manual,
		]
	);
}
