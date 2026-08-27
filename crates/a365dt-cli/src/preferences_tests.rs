use std::{fs, num::NonZeroUsize, process, time::SystemTime};

use pretty_assertions::assert_eq;

use super::{
	Inspection, Overrides, Preferences, Snapshot, Source, Sources, Store,
};
use crate::error::Error;

#[test]
fn resolves_sparse_download_preferences_with_cli_precedence() {
	let fixture = Fixture::new("precedence");
	fs::write(
		fixture.application_home.join("config.toml"),
		"output = \"~/Videos\"\njobs = 8\n",
	)
	.unwrap();
	let store = fixture.store();

	let actual = store
		.load(Overrides {
			output: None,
			jobs: NonZeroUsize::new(2),
			mux: true,
		})
		.unwrap();

	assert_eq!(
		actual,
		Preferences {
			output: fixture.home.join("Videos"),
			jobs: NonZeroUsize::new(2).unwrap(),
			mux: true,
			adult: false,
			adult_telemetry: false,
			auto_play_next_episode: false,
		}
	);
}

#[test]
fn rejects_cwd_relative_configured_output() {
	let fixture = Fixture::new("relative-output");
	fs::write(
		fixture.application_home.join("config.toml"),
		"output = \"Videos\"\n",
	)
	.unwrap();

	let error = fixture.store().load(Overrides::default()).unwrap_err();

	assert_eq!(
		error,
		Error::new(
			"The configured output directory must be absolute or start with `~/`."
		)
	);
}

#[test]
fn validates_configured_output_before_applying_cli_overrides() {
	for (name, output) in [("relative", "Videos"), ("bare-home", "~")] {
		let fixture = Fixture::new(name);
		fs::write(
			fixture.application_home.join("config.toml"),
			format!("output = {output:?}\n"),
		)
		.unwrap();

		assert_eq!(
			fixture
				.store()
				.load(Overrides {
					output: Some(fixture.home.join("Anime")),
					..Overrides::default()
				})
				.unwrap_err(),
			Error::new(
				"The configured output directory must be absolute or start with `~/`."
			)
		);
	}
}

#[test]
fn resolves_home_slash_as_the_home_directory() {
	let fixture = Fixture::new("home-slash");
	fs::write(
		fixture.application_home.join("config.toml"),
		"output = \"~/\"\n",
	)
	.unwrap();

	assert_eq!(
		fixture.store().load(Overrides::default()).unwrap().output,
		fixture.home
	);
}

#[test]
fn prepares_a_tui_configured_output_directory() {
	let fixture = Fixture::new("tui-output");
	let expected = fixture.home.join("Anime");

	let actual = fixture
		.store()
		.prepare_configured_output("~/Anime")
		.unwrap();

	assert_eq!(actual, fs::canonicalize(expected).unwrap());
}

#[test]
fn saves_loads_and_resets_download_preferences() {
	let fixture = Fixture::new("lifecycle");
	let store = fixture.store();
	let expected = Preferences {
		output: fixture.home.join("Anime"),
		jobs: NonZeroUsize::new(8).unwrap(),
		mux: true,
		adult: true,
		adult_telemetry: true,
		auto_play_next_episode: true,
	};

	store.save(&expected).unwrap();
	assert_eq!(store.load(Overrides::default()).unwrap(), expected);
	store.reset().unwrap();
	store.reset().unwrap();

	assert_eq!(
		store.load(Overrides::default()).unwrap(),
		Preferences {
			output: fixture.current_directory.clone(),
			jobs: NonZeroUsize::new(4).unwrap(),
			mux: false,
			adult: false,
			adult_telemetry: false,
			auto_play_next_episode: false,
		}
	);
}

#[test]
fn reports_the_source_of_every_download_preference() {
	let fixture = Fixture::new("sources");
	let path = fixture.application_home.join("config.toml");
	fs::write(&path, "jobs = 8\n").unwrap();

	assert_eq!(
		fixture.store().inspect(),
		Inspection::Ready {
			path,
			snapshot: Snapshot {
				preferences: Preferences {
					output: fixture.current_directory.clone(),
					jobs: NonZeroUsize::new(8).unwrap(),
					mux: false,
					adult: false,
					adult_telemetry: false,
					auto_play_next_episode: false,
				},
				sources: Sources {
					output: Source::BuiltIn,
					jobs: Source::Config,
					mux: Source::BuiltIn,
					adult: Source::BuiltIn,
					adult_telemetry: Source::BuiltIn,
					auto_play_next_episode: Source::BuiltIn,
				},
			},
		}
	);
}

#[test]
fn resolves_content_privacy_and_playback_preferences_from_config() {
	let fixture = Fixture::new("content-playback");
	fs::write(
		fixture.application_home.join("config.toml"),
		concat!(
			"adult = true\n",
			"adult_telemetry = true\n",
			"auto_play_next_episode = true\n",
		),
	)
	.unwrap();

	assert_eq!(
		fixture.store().load(Overrides::default()).unwrap(),
		Preferences {
			output: fixture.current_directory.clone(),
			jobs: NonZeroUsize::new(4).unwrap(),
			mux: false,
			adult: true,
			adult_telemetry: true,
			auto_play_next_episode: true,
		}
	);
}

#[test]
fn rejects_unknown_keys_and_invalid_values() {
	for (name, contents) in [("unknown", "job = 8\n"), ("zero", "jobs = 0\n")] {
		let fixture = Fixture::new(name);
		fs::write(fixture.application_home.join("config.toml"), contents)
			.unwrap();

		assert!(matches!(
			fixture.store().inspect(),
			Inspection::Invalid { .. }
		));
	}
}

#[test]
fn rejects_output_inside_the_application_home() {
	let fixture = Fixture::new("owned-output");
	fs::write(
		fixture.application_home.join("config.toml"),
		format!(
			"output = {:?}\n",
			fixture
				.application_home
				.join("downloads")
				.display()
				.to_string()
		),
	)
	.unwrap();

	assert_eq!(
		fixture.store().load(Overrides::default()).unwrap_err(),
		Error::new(
			"The output directory cannot be inside the Application home."
		)
	);
}

struct Fixture {
	root: std::path::PathBuf,
	application_home: std::path::PathBuf,
	home: std::path::PathBuf,
	current_directory: std::path::PathBuf,
}

impl Fixture {
	fn new(name: &str) -> Self {
		let root = std::env::temp_dir().join(format!(
			"a365-preferences-{name}-{}-{}",
			process::id(),
			SystemTime::now()
				.duration_since(SystemTime::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		let application_home = root.join(".a365");
		let home = root.join("home");
		let current_directory = root.join("working");
		for directory in [&application_home, &home, &current_directory] {
			fs::create_dir_all(directory).unwrap();
		}
		Self {
			root,
			application_home,
			home,
			current_directory,
		}
	}

	fn store(&self) -> Store {
		Store {
			application_home: self.application_home.clone(),
			home: self.home.clone(),
			current_directory: self.current_directory.clone(),
		}
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		fs::remove_dir_all(&self.root).unwrap();
	}
}
