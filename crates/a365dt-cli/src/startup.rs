use std::{
	collections::hash_map::RandomState,
	hash::BuildHasher,
	io::{self, IsTerminal},
	path::{Path, PathBuf},
	process,
	time::{Duration, SystemTime},
};

use console::{Style, strip_ansi_codes, style};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use semver::Version;

use crate::{
	cache::{CompletedRelease, Release, ReleaseState, Store},
	error::Error,
	ui,
};

const TIPS: &str = include_str!("../tips.txt");
const LATEST_RELEASE_URL: &str =
	"https://api.github.com/repos/Gridness/a365/releases/latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CHECK_FAILURE: &str = "Could not check for updates.";

#[derive(Default)]
pub(crate) struct Notices {
	update: Option<Update>,
	tip: Option<&'static str>,
}

pub(crate) async fn load(store: &Store) -> Notices {
	if !io::stdout().is_terminal() {
		return Notices::default();
	}
	Notices {
		update: cached_update(store).await,
		tip: random_tip(),
	}
}

pub(crate) fn show(notices: &Notices) {
	println!();
	if let Some(update) = &notices.update {
		show_update(update);
		println!();
	}
	if let Some(tip) = notices.tip {
		println!("{}: {}", style("Tip").bold(), render_markdown(tip));
		println!();
	}
}

pub async fn check(store: &Store) -> Result<Option<Update>, Error> {
	let (release, update) = fetch_update().await?;
	if let Err(error) = store.save_release(release).await {
		ui::warning(error);
	}
	Ok(update)
}

async fn cached_update(store: &Store) -> Option<Update> {
	match store.load_release().await {
		Ok(ReleaseState::Fresh(release)) => update_from_release(release),
		Ok(ReleaseState::Stale(release)) => {
			refresh_update(store, ReleaseFallback::Stale(release)).await
		}
		Ok(ReleaseState::Missing) => {
			refresh_update(store, ReleaseFallback::Missing).await
		}
		Err(error) => {
			ui::warning(error);
			fetch_update().await.ok()?.1
		}
	}
}

async fn refresh_update(
	store: &Store,
	fallback: ReleaseFallback,
) -> Option<Update> {
	let Ok((release, update)) = fetch_update().await else {
		return match fallback {
			ReleaseFallback::Stale(release) => update_from_release(release),
			ReleaseFallback::Missing => None,
		};
	};
	if let Err(error) = store.save_release(release).await {
		ui::warning(error);
	}
	update
}

async fn fetch_update() -> Result<(CompletedRelease, Option<Update>), Error> {
	let release = fetch_release().await?;
	let completed = CompletedRelease::now(release.clone());
	let update = available_update(release.clone())?;
	Ok((completed, update))
}

async fn fetch_release() -> Result<Release, Error> {
	let client = reqwest::Client::builder()
		.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
		.timeout(REQUEST_TIMEOUT)
		.build()
		.map_err(check_error)?;
	let release = client
		.get(LATEST_RELEASE_URL)
		.header("Accept", "application/vnd.github+json")
		.send()
		.await
		.map_err(check_error)?
		.error_for_status()
		.map_err(check_error)?
		.json::<Release>()
		.await
		.map_err(check_error)?;
	Ok(release)
}

fn check_error(error: impl std::fmt::Display) -> Error {
	Error::with_debug(CHECK_FAILURE, error)
}

fn update_from_release(release: Release) -> Option<Update> {
	available_update(release).ok().flatten()
}

enum ReleaseFallback {
	Stale(Release),
	Missing,
}

fn available_update(release: Release) -> Result<Option<Update>, Error> {
	update_from(env!("CARGO_PKG_VERSION"), release)
}

fn update_from(
	installed: &str,
	release: Release,
) -> Result<Option<Update>, Error> {
	let installed = Version::parse(installed).map_err(|error| {
		check_error(format!(
			"Could not parse installed version `{installed}`: {error}"
		))
	})?;
	let available = release
		.tag_name
		.strip_prefix('v')
		.unwrap_or(release.tag_name.as_str());
	let available = Version::parse(available).map_err(|error| {
		check_error(format!(
			"Could not parse release version `{available}`: {error}"
		))
	})?;
	if available <= installed || !available.pre.is_empty() {
		return Ok(None);
	}
	Ok(Some(Update {
		installed,
		available,
		release_url: release.html_url,
	}))
}

pub fn show_update(update: &Update) {
	println!(
		"{} {} {} {}",
		style("💫 Upgrade available:").blue().bold(),
		style(format!("v{}", update.installed)).white(),
		style("→").green(),
		style(format!("v{}", update.available)).white()
	);
	println!(
		"   Upgrade: {}",
		upgrade_instruction(
			installation_channel(),
			update.release_url.as_str()
		)
	);
}

fn upgrade_instruction(
	channel: InstallationChannel,
	release_url: &str,
) -> String {
	match channel {
		InstallationChannel::Homebrew => {
			"brew upgrade Gridness/oosama/a365".to_owned()
		}
		InstallationChannel::Cargo => concat!(
			"cargo install --git https://github.com/Gridness/a365 ",
			"--bin a365"
		)
		.to_owned(),
		InstallationChannel::Manual => {
			format!("Download {release_url} and replace a365.")
		}
	}
}

fn installation_channel() -> InstallationChannel {
	let Ok(executable) = std::env::current_exe() else {
		return InstallationChannel::Manual;
	};
	installation_channel_from_path(&executable, &cargo_bin_directories())
}

fn installation_channel_from_path(
	executable: &Path,
	cargo_bin_directories: &[PathBuf],
) -> InstallationChannel {
	let executable = executable
		.canonicalize()
		.unwrap_or_else(|_| executable.into());
	if executable
		.ancestors()
		.any(|ancestor| ancestor.ends_with("Cellar/a365"))
	{
		return InstallationChannel::Homebrew;
	}
	let parent = executable.parent();
	if cargo_bin_directories
		.iter()
		.any(|directory| Some(directory.as_path()) == parent)
	{
		return InstallationChannel::Cargo;
	}
	InstallationChannel::Manual
}

fn cargo_bin_directories() -> Vec<PathBuf> {
	let mut directories = Vec::new();
	if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
		directories.push(PathBuf::from(cargo_home).join("bin"));
	}
	if let Some(home) = std::env::var_os("HOME") {
		directories.push(PathBuf::from(home).join(".cargo").join("bin"));
	}
	directories.sort_unstable();
	directories.dedup();
	directories
}

fn random_tip() -> Option<&'static str> {
	let tips = TIPS
		.lines()
		.map(str::trim)
		.filter(|tip| !tip.is_empty())
		.collect::<Vec<_>>();
	if tips.is_empty() {
		return None;
	}
	let now = SystemTime::now()
		.duration_since(SystemTime::UNIX_EPOCH)
		.map_or(0, |duration| duration.as_nanos());
	let index =
		RandomState::new().hash_one((now, process::id())) as usize % tips.len();
	Some(tips[index])
}

impl Notices {
	pub(crate) fn plain_tip(&self) -> Option<String> {
		self.tip
			.map(|tip| strip_ansi_codes(&render_markdown(tip)).into_owned())
	}

	pub(crate) fn tui_update_notice(&self) -> Option<String> {
		self.update.as_ref().map(|update| {
			format!(
				"💫 Upgrade available · v{} → v{} · run `a365 update` for instructions",
				update.installed, update.available,
			)
		})
	}
}

fn render_markdown(markdown: &str) -> String {
	let mut output = String::new();
	let mut state = MarkdownStyle::default();
	for event in Parser::new(markdown) {
		match event {
			Event::Start(Tag::Strong) => state.strong += 1,
			Event::End(TagEnd::Strong) => {
				state.strong = state.strong.saturating_sub(1);
			}
			Event::Start(Tag::Emphasis) => state.emphasis += 1,
			Event::End(TagEnd::Emphasis) => {
				state.emphasis = state.emphasis.saturating_sub(1);
			}
			Event::Start(Tag::Link { dest_url, .. }) => {
				state.links.push(dest_url.into_string());
			}
			Event::End(TagEnd::Link) => {
				if let Some(url) = state.links.pop() {
					output.push_str(" (");
					output.push_str(&url);
					output.push(')');
				}
			}
			Event::Text(text) => {
				push_markdown_text(
					&mut output,
					&text,
					&state,
					MarkdownText::Normal,
				);
			}
			Event::Code(code) => {
				push_markdown_text(
					&mut output,
					&code,
					&state,
					MarkdownText::Code,
				);
			}
			Event::InlineMath(text)
			| Event::DisplayMath(text)
			| Event::Html(text)
			| Event::InlineHtml(text)
			| Event::FootnoteReference(text) => {
				push_markdown_text(
					&mut output,
					&text,
					&state,
					MarkdownText::Normal,
				);
			}
			Event::SoftBreak | Event::HardBreak => output.push(' '),
			Event::Rule => output.push('—'),
			Event::TaskListMarker(checked) => {
				output.push_str(if checked { "[x] " } else { "[ ] " });
			}
			Event::Start(_) | Event::End(_) => {}
		}
	}
	output
}

fn push_markdown_text(
	output: &mut String,
	text: &str,
	state: &MarkdownStyle,
	kind: MarkdownText,
) {
	let mut markdown_style = Style::new();
	if state.strong > 0 {
		markdown_style = markdown_style.bold();
	}
	if state.emphasis > 0 {
		markdown_style = markdown_style.italic();
	}
	if kind == MarkdownText::Code {
		markdown_style = markdown_style.cyan();
	}
	if !state.links.is_empty() {
		markdown_style = markdown_style.blue().underlined();
	}
	output.push_str(&markdown_style.apply_to(text).to_string());
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Update {
	pub installed: Version,
	pub available: Version,
	pub release_url: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstallationChannel {
	Homebrew,
	Cargo,
	Manual,
}

#[derive(Default)]
struct MarkdownStyle {
	strong: usize,
	emphasis: usize,
	links: Vec<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MarkdownText {
	Normal,
	Code,
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
