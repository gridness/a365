use std::{path::PathBuf, process::Stdio, time::Duration};

use serde::Deserialize;
use tokio::{
	fs,
	io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
	net::UnixStream,
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

use proxy::MediaProxy;

#[cfg(target_os = "macos")]
const IINA_CLI: &str = "/Applications/IINA.app/Contents/MacOS/iina-cli";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
	Interrupted,
	NaturalEnd,
	Stopped,
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
	reason: Option<String>,
}

pub(crate) async fn play(
	api: Anime365,
	release: &PlannedRelease,
	title: &str,
	cancelled: watch::Receiver<bool>,
) -> Result<Outcome, Error> {
	let proxy = MediaProxy::start(api, release).await?;
	play_with_proxy(proxy, title, cancelled).await
}

pub(crate) async fn play_public(
	url: String,
	title: &str,
	cancelled: watch::Receiver<bool>,
) -> Result<Outcome, Error> {
	let proxy = MediaProxy::start_public(url).await?;
	play_with_proxy(proxy, title, cancelled).await
}

async fn play_with_proxy(
	proxy: MediaProxy,
	title: &str,
	mut cancelled: watch::Receiver<bool>,
) -> Result<Outcome, Error> {
	let socket = socket_path();
	let command = player_command(
		&proxy.media_url,
		proxy.subtitle_url.as_deref(),
		title,
		&socket,
	)?;
	let mut child = spawn(&command)?;
	let monitor = monitor(socket.clone());
	let outcome = tokio::select! {
		result = wait(&mut child, monitor) => result,
		changed = cancelled.changed() => {
			let _ = changed;
			let _ = request_player_quit(&socket).await;
			child.kill().await.map_err(|error| {
				Error::with_debug("Could not stop the Player.", error)
			})?;
			Ok(Outcome::Interrupted)
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
	monitor: impl std::future::Future<Output = Option<String>>,
) -> Result<Outcome, Error> {
	tokio::pin!(monitor);
	let mut monitor_done = false;
	loop {
		tokio::select! {
			observed = &mut monitor, if !monitor_done => {
				monitor_done = true;
				if let Some(observed) = observed {
					child.kill().await.map_err(|error| {
						Error::with_debug("Could not finish the Player process.", error)
					})?;
					return outcome_from_end_reason(&observed);
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
				return Ok(Outcome::Stopped);
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

async fn monitor(path: PathBuf) -> Option<String> {
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
	let mut lines = BufReader::new(stream?).lines();
	while let Ok(Some(line)) = lines.next_line().await {
		let Ok(event) = serde_json::from_str::<PlayerEvent>(&line) else {
			continue;
		};
		if event.event.as_deref() == Some("end-file") {
			return event.reason;
		}
	}
	None
}

fn player_command(
	media_url: &str,
	subtitle_url: Option<&str>,
	title: &str,
	socket: &std::path::Path,
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
		))
	}
}

fn build_player_command(
	kind: PlayerKind,
	program: PathBuf,
	media_url: &str,
	subtitle_url: Option<&str>,
	title: &str,
	socket: &str,
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
	if let Some(subtitle_url) = subtitle_url {
		arguments.push(match kind {
			#[cfg(any(test, target_os = "macos"))]
			PlayerKind::Iina => format!("--mpv-sub-file={subtitle_url}"),
			#[cfg(any(test, not(target_os = "macos")))]
			PlayerKind::Mpv => format!("--sub-file={subtitle_url}"),
		});
	}
	arguments.push(media_url.into());
	PlayerCommand { program, arguments }
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

#[cfg(test)]
#[path = "playback_tests.rs"]
mod tests;
