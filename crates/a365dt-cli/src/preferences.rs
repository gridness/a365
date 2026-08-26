use std::{
	fs::{self, File},
	io::Write,
	num::NonZeroUsize,
	path::{Component, Path, PathBuf},
	process,
	time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{app_files, error::Error, ui};

mod command;

pub(crate) use command::{ConfigCommand, ResetPermission};

const DEFAULT_JOBS: usize = 4;
const JOBS_WARNING_THRESHOLD: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Preferences {
	pub output: PathBuf,
	pub jobs: NonZeroUsize,
	pub mux: bool,
	pub adult: bool,
	pub adult_telemetry: bool,
	pub auto_play_next_episode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdultContent {
	Hidden,
	Visible,
}

impl Preferences {
	pub(crate) const fn adult_content(&self) -> AdultContent {
		if self.adult {
			AdultContent::Visible
		} else {
			AdultContent::Hidden
		}
	}
}

#[derive(Default)]
pub(crate) struct Overrides {
	pub output: Option<PathBuf>,
	pub jobs: Option<NonZeroUsize>,
	pub mux: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
	BuiltIn,
	Config,
	CommandLine,
}

#[derive(Debug, Eq, PartialEq)]
struct Sources {
	output: Source,
	jobs: Source,
	mux: Source,
	adult: Source,
	adult_telemetry: Source,
	auto_play_next_episode: Source,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Snapshot {
	pub preferences: Preferences,
	sources: Sources,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Inspection {
	Missing { path: PathBuf, snapshot: Snapshot },
	Ready { path: PathBuf, snapshot: Snapshot },
	Invalid { path: PathBuf, error: Error },
	Unreadable { path: PathBuf, error: Error },
}

pub(crate) struct Store {
	application_home: PathBuf,
	home: PathBuf,
	current_directory: PathBuf,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FilePreferences {
	output: Option<String>,
	jobs: Option<NonZeroUsize>,
	mux: Option<bool>,
	adult: Option<bool>,
	adult_telemetry: Option<bool>,
	auto_play_next_episode: Option<bool>,
}

enum FileState {
	Missing,
	Ready(FilePreferences),
	Invalid(Error),
	Unreadable(Error),
}

impl Store {
	pub(crate) fn discover() -> Result<Self, Error> {
		let application_home =
			app_files::application_home().ok_or_else(|| {
				Error::new("Could not resolve the user Application home.")
			})?;
		let home = application_home
			.parent()
			.ok_or_else(|| {
				Error::new("Could not resolve the user home directory.")
			})?
			.to_owned();
		let current_directory = std::env::current_dir().map_err(|error| {
			Error::with_debug("Could not resolve the current directory.", error)
		})?;
		Ok(Self {
			application_home,
			home,
			current_directory,
		})
	}

	pub(crate) fn load(
		&self,
		overrides: Overrides,
	) -> Result<Preferences, Error> {
		match self.state() {
			FileState::Missing => {
				self.resolve(FilePreferences::default(), overrides)
			}
			FileState::Ready(file) => self.resolve(file, overrides),
			FileState::Invalid(error) | FileState::Unreadable(error) => {
				Err(error)
			}
		}
		.map(|snapshot| snapshot.preferences)
	}

	pub(crate) fn inspect(&self) -> Inspection {
		let path = self.file();
		match self.state() {
			FileState::Missing => {
				match self
					.resolve(FilePreferences::default(), Overrides::default())
				{
					Ok(snapshot) => Inspection::Missing { path, snapshot },
					Err(error) => Inspection::Invalid { path, error },
				}
			}
			FileState::Ready(file) => {
				match self.resolve(file, Overrides::default()) {
					Ok(snapshot) => Inspection::Ready { path, snapshot },
					Err(error) => Inspection::Invalid { path, error },
				}
			}
			FileState::Invalid(error) => Inspection::Invalid { path, error },
			FileState::Unreadable(error) => {
				Inspection::Unreadable { path, error }
			}
		}
	}

	pub(crate) fn prepare_output(
		&self,
		output: &Path,
	) -> Result<PathBuf, Error> {
		fs::create_dir_all(output).map_err(|error| {
			Error::with_debug(
				format!(
					"Could not create output directory {}.",
					output.display()
				),
				error,
			)
		})?;
		let output = fs::canonicalize(output).map_err(|error| {
			Error::with_debug(
				format!(
					"Could not inspect output directory {}.",
					output.display()
				),
				error,
			)
		})?;
		self.ensure_outside_application_home(&output)?;
		Ok(output)
	}

	fn resolve(
		&self,
		file: FilePreferences,
		overrides: Overrides,
	) -> Result<Snapshot, Error> {
		let configured_output = file
			.output
			.map(|output| self.resolve_configured_output(output))
			.transpose()?;
		if let Some(output) = &configured_output {
			self.ensure_outside_application_home(output)?;
		}
		let (output, output_source) =
			match (overrides.output, configured_output) {
				(Some(output), _) => {
					(self.resolve_cli_output(output), Source::CommandLine)
				}
				(None, Some(output)) => (output, Source::Config),
				(None, None) => {
					(self.current_directory.clone(), Source::BuiltIn)
				}
			};
		self.ensure_outside_application_home(&output)?;
		let (jobs, jobs_source) = match (overrides.jobs, file.jobs) {
			(Some(jobs), _) => (jobs, Source::CommandLine),
			(None, Some(jobs)) => (jobs, Source::Config),
			(None, None) => {
				(NonZeroUsize::new(DEFAULT_JOBS).unwrap(), Source::BuiltIn)
			}
		};
		let (mux, mux_source) = if overrides.mux {
			(true, Source::CommandLine)
		} else if let Some(mux) = file.mux {
			(mux, Source::Config)
		} else {
			(false, Source::BuiltIn)
		};
		let (adult, adult_source) = configured_bool(file.adult);
		let (adult_telemetry, adult_telemetry_source) =
			configured_bool(file.adult_telemetry);
		let (auto_play_next_episode, auto_play_next_episode_source) =
			configured_bool(file.auto_play_next_episode);
		Ok(Snapshot {
			preferences: Preferences {
				output,
				jobs,
				mux,
				adult,
				adult_telemetry,
				auto_play_next_episode,
			},
			sources: Sources {
				output: output_source,
				jobs: jobs_source,
				mux: mux_source,
				adult: adult_source,
				adult_telemetry: adult_telemetry_source,
				auto_play_next_episode: auto_play_next_episode_source,
			},
		})
	}

	fn state(&self) -> FileState {
		let path = self.file();
		let contents = match fs::read_to_string(&path) {
			Ok(contents) => contents,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				return FileState::Missing;
			}
			Err(error) => {
				return FileState::Unreadable(Error::new(format!(
					"Could not read {}: {error}",
					path.display()
				)));
			}
		};
		match toml::from_str(&contents) {
			Ok(file) => FileState::Ready(file),
			Err(error) => FileState::Invalid(Error::new(format!(
				"Could not parse {}: {error}",
				path.display()
			))),
		}
	}

	fn save(&self, preferences: &Preferences) -> Result<(), Error> {
		let path = self.file();
		let output = preferences.output.to_str().ok_or_else(|| {
			Error::new("The output directory cannot be encoded in TOML.")
		})?;
		let contents = toml::to_string(&FilePreferences {
			output: Some(output.to_owned()),
			jobs: Some(preferences.jobs),
			mux: Some(preferences.mux),
			adult: Some(preferences.adult),
			adult_telemetry: Some(preferences.adult_telemetry),
			auto_play_next_episode: Some(preferences.auto_play_next_episode),
		})
		.map_err(|error| {
			Error::with_debug("Could not encode preferences.", error)
		})?;
		let temporary = path.with_file_name(format!(
			"config.toml.{}.{}.tmp",
			process::id(),
			SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap_or_default()
				.as_nanos()
		));
		// ponytail: atomic last-write-wins; add a lock if concurrent editing matters.
		let write = (|| {
			let mut file = File::create(&temporary)?;
			app_files::private_file(&temporary)?;
			file.write_all(contents.as_bytes())?;
			file.sync_all()?;
			fs::rename(&temporary, &path)
		})();
		if let Err(error) = write {
			let _ = fs::remove_file(&temporary);
			return Err(Error::with_debug(
				format!("Could not write {}.", path.display()),
				error,
			));
		}
		Ok(())
	}

	fn reset(&self) -> Result<(), Error> {
		let path = self.file();
		match fs::remove_file(&path) {
			Ok(()) => Ok(()),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
				Ok(())
			}
			Err(error) => Err(Error::with_debug(
				format!("Could not remove {}.", path.display()),
				error,
			)),
		}
	}

	fn resolve_cli_output(&self, output: PathBuf) -> PathBuf {
		if output.is_absolute() {
			output
		} else {
			self.current_directory.join(output)
		}
	}

	fn resolve_configured_output(
		&self,
		output: String,
	) -> Result<PathBuf, Error> {
		let path = PathBuf::from(&output);
		if path.is_absolute() {
			return Ok(path);
		}
		if output == "~/" {
			return Ok(self.home.clone());
		}
		if let Some(output) = output.strip_prefix("~/") {
			return Ok(self.home.join(output));
		}
		Err(Error::new(
			"The configured output directory must be absolute or start with `~/`.",
		))
	}

	fn ensure_outside_application_home(
		&self,
		output: &Path,
	) -> Result<(), Error> {
		let output = resolved_path(output);
		let application_home = resolved_path(&self.application_home);
		if output.starts_with(application_home) {
			Err(Error::new(
				"The output directory cannot be inside the Application home.",
			))
		} else {
			Ok(())
		}
	}

	fn file(&self) -> PathBuf {
		self.application_home.join("config.toml")
	}
}

fn configured_bool(value: Option<bool>) -> (bool, Source) {
	match value {
		Some(value) => (value, Source::Config),
		None => (false, Source::BuiltIn),
	}
}

pub(crate) fn warn_if_high_concurrency(preferences: &Preferences) {
	if preferences.jobs.get() > JOBS_WARNING_THRESHOLD {
		ui::warning(format!(
			"{} concurrent downloads may trigger Anime365 rate limits.",
			preferences.jobs
		));
	}
}

fn normalized(path: &Path) -> PathBuf {
	let mut normalized = PathBuf::new();
	for component in path.components() {
		match component {
			Component::CurDir => {}
			Component::ParentDir => {
				normalized.pop();
			}
			Component::Prefix(_)
			| Component::RootDir
			| Component::Normal(_) => {
				normalized.push(component.as_os_str());
			}
		}
	}
	normalized
}

fn resolved_path(path: &Path) -> PathBuf {
	let mut ancestor = path;
	let mut suffix = Vec::new();
	loop {
		if let Ok(mut resolved) = fs::canonicalize(ancestor) {
			for component in suffix.iter().rev() {
				resolved.push(component);
			}
			return normalized(&resolved);
		}
		let Some(name) = ancestor.file_name() else {
			return normalized(path);
		};
		suffix.push(name);
		let Some(parent) = ancestor.parent() else {
			return normalized(path);
		};
		ancestor = parent;
	}
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
