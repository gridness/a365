use std::{
	fmt,
	path::PathBuf,
	process::Stdio,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
	time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::{
	fs,
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::{
		UnixStream,
		unix::{OwnedReadHalf, OwnedWriteHalf},
	},
	process::{Child, Command},
	sync::watch,
	time::sleep,
};
use uuid::Uuid;

use crate::{
	api::{Anime365, Episode},
	error::Error,
	select::PlannedRelease,
};

mod proxy;
#[cfg(target_os = "macos")]
mod subtitle;

use proxy::MediaProxy;
#[cfg(target_os = "macos")]
use subtitle::LocalSubtitle;

#[cfg(target_os = "macos")]
const IINA_CLI: &str = "/Applications/IINA.app/Contents/MacOS/iina-cli";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
	Interrupted,
	NaturalEnd,
	Stopped,
}

#[derive(
	Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[serde(transparent)]
pub(crate) struct Position(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Report {
	pub outcome: Outcome,
	pub position: Position,
}

impl Position {
	pub(crate) const START: Self = Self(0);

	pub(crate) const fn from_seconds(seconds: u64) -> Self {
		Self(seconds)
	}

	pub(crate) const fn seconds(self) -> u64 {
		self.0
	}

	pub(crate) const fn at_start(&self) -> bool {
		self.0 == 0
	}
}

impl fmt::Display for Position {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let hours = self.0 / 3_600;
		let minutes = (self.0 % 3_600) / 60;
		let seconds = self.0 % 60;
		if hours == 0 {
			write!(formatter, "{minutes}:{seconds:02}")
		} else {
			write!(formatter, "{hours}:{minutes:02}:{seconds:02}")
		}
	}
}

pub(crate) fn inspect_player() -> Result<PathBuf, Error> {
	#[cfg(target_os = "macos")]
	{
		find_iina().ok_or_else(|| {
			Error::new("IINA was not found in PATH or /Applications/IINA.app.")
		})
	}
	#[cfg(not(target_os = "macos"))]
	{
		find_in_path("mpv")
			.ok_or_else(|| Error::new("mpv was not found in PATH."))
	}
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlayerCommand {
	program: PathBuf,
	arguments: Vec<String>,
	ipc_subtitle_source: Option<String>,
}

#[derive(Clone, Copy)]
enum PlayerKind {
	#[cfg(any(test, target_os = "macos"))]
	Iina,
	#[cfg(any(test, not(target_os = "macos")))]
	Mpv,
}

#[derive(Deserialize)]
struct PlayerEvent {
	event: Option<String>,
	name: Option<String>,
	reason: Option<String>,
	request_id: Option<u64>,
	error: Option<String>,
	data: Option<serde_json::Value>,
}

pub(crate) async fn play(
	api: Anime365,
	release: &PlannedRelease,
	title: &str,
	position: Position,
	cancelled: watch::Receiver<bool>,
) -> Result<Report, Error> {
	let proxy = MediaProxy::start(api, release).await?;
	play_with_proxy(proxy, title, position, cancelled).await
}

pub(crate) async fn play_public(
	url: String,
	title: &str,
	cancelled: watch::Receiver<bool>,
) -> Result<Report, Error> {
	let proxy = MediaProxy::start_public(url).await?;
	play_with_proxy(proxy, title, Position::START, cancelled).await
}

async fn play_with_proxy(
	proxy: MediaProxy,
	title: &str,
	position: Position,
	mut cancelled: watch::Receiver<bool>,
) -> Result<Report, Error> {
	#[cfg(target_os = "macos")]
	let local_subtitle = match proxy.subtitle_url.as_deref() {
		Some(url) => Some(LocalSubtitle::fetch(url).await?),
		None => None,
	};
	#[cfg(target_os = "macos")]
	let subtitle_source = local_subtitle
		.as_ref()
		.map(|subtitle| subtitle.path().to_string_lossy().into_owned());
	#[cfg(not(target_os = "macos"))]
	let subtitle_source = proxy.subtitle_url.clone();
	let socket = socket_path();
	let command = player_command(
		&proxy.media_url,
		subtitle_source.as_deref(),
		title,
		&socket,
		position,
	)?;
	let mut child = spawn(&command)?;
	let progress = Arc::new(AtomicU64::new(position.seconds()));
	let monitor = monitor(
		socket.clone(),
		command.ipc_subtitle_source.clone(),
		Arc::clone(&progress),
	);
	let outcome = tokio::select! {
		result = wait(&mut child, monitor, Arc::clone(&progress)) => result,
		changed = cancelled.changed() => {
			let _ = changed;
			let _ = request_player_quit(&socket).await;
			child.kill().await.map_err(|error| {
				Error::with_debug("Could not stop the Player.", error)
			})?;
			Ok(Report {
				outcome: Outcome::Interrupted,
				position: current_position(&progress),
			})
		}
	};
	let _ = fs::remove_file(socket).await;
	drop(proxy);
	outcome
}

async fn request_player_quit(path: &std::path::Path) -> std::io::Result<()> {
	let mut stream = UnixStream::connect(path).await?;
	stream.write_all(b"{\"command\":[\"quit\"]}\n").await
}

pub(crate) fn next_whole_episode<'a>(
	episodes: &'a [Episode],
	current: &Episode,
) -> Option<&'a Episode> {
	let current = whole_number(current)?;
	episodes
		.iter()
		.filter_map(|episode| {
			whole_number(episode).map(|number| (number, episode))
		})
		.find(|(number, _)| *number == current.saturating_add(1))
		.map(|(_, episode)| episode)
}

async fn wait(
	child: &mut Child,
	monitor: impl std::future::Future<Output = Result<Option<String>, Error>>,
	progress: Arc<AtomicU64>,
) -> Result<Report, Error> {
	tokio::pin!(monitor);
	let mut monitor_done = false;
	loop {
		tokio::select! {
			observed = &mut monitor, if !monitor_done => {
				monitor_done = true;
				if let Some(observed) = observed? {
					child.kill().await.map_err(|error| {
						Error::with_debug("Could not finish the Player process.", error)
					})?;
					return Ok(Report {
						outcome: outcome_from_end_reason(&observed)?,
						position: current_position(&progress),
					});
				}
			}
			status = child.wait() => {
				let status = status.map_err(|error| {
					Error::with_debug("Could not wait for the Player.", error)
				})?;
				if !status.success() {
					return Err(Error::new(format!(
						"The Player exited with {status}."
					)));
				}
				return Ok(Report {
					outcome: Outcome::Stopped,
					position: current_position(&progress),
				});
			}
		}
	}
}

fn outcome_from_end_reason(reason: &str) -> Result<Outcome, Error> {
	match reason {
		"eof" => Ok(Outcome::NaturalEnd),
		"error" => Err(Error::new("The Player reported a playback error.")),
		"stop" | "quit" | "redirect" | "unknown" => Ok(Outcome::Stopped),
		_ => Ok(Outcome::Stopped),
	}
}

async fn monitor(
	path: PathBuf,
	subtitle_source: Option<String>,
	progress: Arc<AtomicU64>,
) -> Result<Option<String>, Error> {
	let mut stream = None;
	for _ in 0..50 {
		match UnixStream::connect(&path).await {
			Ok(connected) => {
				stream = Some(connected);
				break;
			}
			Err(_) => sleep(Duration::from_millis(100)).await,
		}
	}
	let Some(stream) = stream else {
		return if subtitle_source.is_some() {
			Err(Error::new(
				"Could not connect to the Player to load subtitles.",
			))
		} else {
			Ok(None)
		};
	};
	let (read, mut write) = stream.into_split();
	let mut lines = BufReader::new(read).lines();
	if let Some(source) = subtitle_source {
		load_subtitle(&mut lines, &mut write, &source).await?;
	}
	let observation = player_request(
		&mut lines,
		&mut write,
		100,
		serde_json::json!(["observe_property", 1, "time-pos"]),
	)
	.await;
	if !matches!(
		observation,
		Ok(PlayerEvent {
			error: Some(error),
			..
		}) if error == "success"
	) {
		return Ok(None);
	}
	while let Ok(Some(line)) = lines.next_line().await {
		let Ok(event) = serde_json::from_str::<PlayerEvent>(&line) else {
			continue;
		};
		if event.event.as_deref() == Some("end-file") {
			return Ok(event.reason);
		}
		if event.event.as_deref() == Some("property-change")
			&& event.name.as_deref() == Some("time-pos")
			&& let Some(seconds) = event.data.and_then(|value| value.as_f64())
			&& seconds.is_finite()
			&& seconds >= 0.0
		{
			progress.store(seconds as u64, Ordering::Relaxed);
		}
	}
	Ok(None)
}

type PlayerLines = tokio::io::Lines<BufReader<OwnedReadHalf>>;

async fn load_subtitle(
	lines: &mut PlayerLines,
	write: &mut OwnedWriteHalf,
	source: &str,
) -> Result<(), Error> {
	let mut request_id = 1;
	let ready = loop {
		let response = player_request(
			lines,
			write,
			request_id,
			serde_json::json!(["get_property", "path"]),
		)
		.await?;
		if response.error.as_deref() == Some("success")
			&& response.data.is_some_and(|data| data.is_string())
		{
			break true;
		}
		if request_id == 50 {
			break false;
		}
		request_id += 1;
		sleep(Duration::from_millis(100)).await;
	};
	if !ready {
		return Err(Error::new(
			"The Player did not become ready to load subtitles.",
		));
	}
	request_id += 1;
	let response = player_request(
		lines,
		write,
		request_id,
		serde_json::json!(["sub-add", source, "select"]),
	)
	.await?;
	if response.error.as_deref() != Some("success") {
		return Err(Error::new(
			"The Player could not load the selected subtitles.",
		));
	}
	Ok(())
}

async fn player_request(
	lines: &mut PlayerLines,
	write: &mut OwnedWriteHalf,
	request_id: u64,
	command: serde_json::Value,
) -> Result<PlayerEvent, Error> {
	let mut request = serde_json::to_vec(&serde_json::json!({
		"command": command,
		"request_id": request_id,
	}))
	.map_err(|error| {
		Error::with_debug("Could not prepare a Player request.", error)
	})?;
	request.push(b'\n');
	write.write_all(&request).await.map_err(|error| {
		Error::with_debug("Could not send a request to the Player.", error)
	})?;
	while let Some(line) = lines.next_line().await.map_err(|error| {
		Error::with_debug("Could not read the Player response.", error)
	})? {
		let Ok(response) = serde_json::from_str::<PlayerEvent>(&line) else {
			continue;
		};
		if response.request_id == Some(request_id) {
			return Ok(response);
		}
	}
	Err(Error::new(
		"The Player closed before loading the selected subtitles.",
	))
}

fn player_command(
	media_url: &str,
	subtitle_url: Option<&str>,
	title: &str,
	socket: &std::path::Path,
	position: Position,
) -> Result<PlayerCommand, Error> {
	let socket = socket.to_string_lossy();
	#[cfg(target_os = "macos")]
	{
		let program = inspect_player().map_err(|_| {
			Error::new(
				"IINA is required for Playback on macOS. Install IINA and expose iina-cli in PATH or /Applications/IINA.app.",
			)
		})?;
		Ok(build_player_command(
			PlayerKind::Iina,
			program,
			media_url,
			subtitle_url,
			title,
			&socket,
			position,
		))
	}
	#[cfg(not(target_os = "macos"))]
	{
		let program = inspect_player().map_err(|_| {
			Error::new(
				"mpv is required for Playback on Linux. Install mpv and ensure it is in PATH.",
			)
		})?;
		Ok(build_player_command(
			PlayerKind::Mpv,
			program,
			media_url,
			subtitle_url,
			title,
			&socket,
			position,
		))
	}
}

fn build_player_command(
	kind: PlayerKind,
	program: PathBuf,
	media_url: &str,
	subtitle_source: Option<&str>,
	title: &str,
	socket: &str,
	position: Position,
) -> PlayerCommand {
	let mut arguments = match kind {
		#[cfg(any(test, target_os = "macos"))]
		PlayerKind::Iina => vec![
			"--no-stdin".into(),
			"--keep-running".into(),
			format!("--mpv-input-ipc-server={socket}"),
			format!("--mpv-force-media-title={title}"),
		],
		#[cfg(any(test, not(target_os = "macos")))]
		PlayerKind::Mpv => vec![
			format!("--input-ipc-server={socket}"),
			format!("--force-media-title={title}"),
		],
	};
	if !position.at_start() {
		arguments.push(match kind {
			#[cfg(any(test, target_os = "macos"))]
			PlayerKind::Iina => {
				format!("--mpv-start={}", position.seconds())
			}
			#[cfg(any(test, not(target_os = "macos")))]
			PlayerKind::Mpv => format!("--start={}", position.seconds()),
		});
	}
	let ipc_subtitle_source = match kind {
		#[cfg(any(test, target_os = "macos"))]
		PlayerKind::Iina => subtitle_source.map(str::to_owned),
		#[cfg(any(test, not(target_os = "macos")))]
		PlayerKind::Mpv => {
			if let Some(subtitle_source) = subtitle_source {
				arguments.push(format!("--sub-file={subtitle_source}"));
			}
			None
		}
	};
	arguments.push(media_url.into());
	PlayerCommand {
		program,
		arguments,
		ipc_subtitle_source,
	}
}

fn spawn(command: &PlayerCommand) -> Result<Child, Error> {
	let mut process = Command::new(&command.program);
	process
		.args(&command.arguments)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()
		.map_err(|error| {
			Error::with_debug("Could not start the Player.", error)
		})
}

#[cfg(target_os = "macos")]
fn find_iina() -> Option<PathBuf> {
	find_in_path("iina-cli").or_else(|| {
		std::path::Path::new(IINA_CLI)
			.is_file()
			.then(|| PathBuf::from(IINA_CLI))
	})
}

fn find_in_path(name: &str) -> Option<PathBuf> {
	std::env::var_os("PATH").and_then(|path| {
		std::env::split_paths(&path)
			.map(|directory| directory.join(name))
			.find(|candidate| candidate.is_file())
	})
}

fn socket_path() -> PathBuf {
	std::env::temp_dir().join(format!("a365-{}.sock", Uuid::now_v7().simple()))
}

fn whole_number(episode: &Episode) -> Option<u64> {
	let number = episode.episode_int.parse::<f64>().ok()?;
	(number.is_finite() && number >= 0.0 && number.fract() == 0.0)
		.then_some(number as u64)
}

fn current_position(progress: &AtomicU64) -> Position {
	Position::from_seconds(progress.load(Ordering::Relaxed))
}

#[cfg(test)]
#[path = "playback_tests.rs"]
mod tests;
