use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::{
	sync::{Arc, atomic::AtomicU64},
	time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{
	Outcome, PlayerKind, Position, Report, build_player_command,
	next_whole_episode, outcome_from_end_reason, request_player_quit, wait,
};
use crate::{api::Episode, content::ContentSource};

fn episode(id: u64, number: &str) -> Episode {
	Episode {
		source: ContentSource::Anime365,
		id,
		episode_int: number.into(),
		episode_full: format!("Episode {number}"),
	}
}

#[test]
fn player_commands_attach_native_ass_from_each_platform_source() {
	let media = "http://127.0.0.1:43815/capability/media";
	let iina_subtitle = "/tmp/a365-subtitle.ass";
	let mpv_subtitle = "http://127.0.0.1:43815/capability/subtitle";
	let socket = "/tmp/a365-player.sock";
	let position = Position::from_seconds(12 * 60 + 34);

	assert_eq!(
		[
			build_player_command(
				PlayerKind::Iina,
				"iina-cli".into(),
				media,
				Some(iina_subtitle),
				"Series — Episode 1",
				socket,
				position,
			),
			build_player_command(
				PlayerKind::Mpv,
				"mpv".into(),
				media,
				Some(mpv_subtitle),
				"Series — Episode 1",
				socket,
				position,
			),
		],
		[
			super::PlayerCommand {
				program: "iina-cli".into(),
				arguments: vec![
					"--no-stdin".into(),
					"--keep-running".into(),
					format!("--mpv-input-ipc-server={socket}"),
					"--mpv-force-media-title=Series — Episode 1".into(),
					"--mpv-start=754".into(),
					media.into(),
				],
				ipc_subtitle_source: Some(iina_subtitle.into()),
			},
			super::PlayerCommand {
				program: "mpv".into(),
				arguments: vec![
					format!("--input-ipc-server={socket}"),
					"--force-media-title=Series — Episode 1".into(),
					"--start=754".into(),
					format!("--sub-file={mpv_subtitle}"),
					media.into(),
				],
				ipc_subtitle_source: None,
			},
		]
	);
}

#[test]
fn automatic_continuation_skips_fractional_episodes() {
	let episodes = vec![
		episode(1, "1"),
		episode(2, "1.5"),
		episode(3, "2"),
		episode(4, "4"),
	];

	assert_eq!(
		next_whole_episode(&episodes, &episodes[0]),
		Some(&episodes[2])
	);
	assert_eq!(next_whole_episode(&episodes, &episodes[2]), None);
	assert_eq!(next_whole_episode(&episodes, &episodes[3]), None);
}

#[test]
fn end_file_reasons_distinguish_continuation_from_stops_and_errors() {
	assert_eq!(
		[
			outcome_from_end_reason("eof"),
			outcome_from_end_reason("stop"),
			outcome_from_end_reason("quit"),
			outcome_from_end_reason("error"),
		],
		[
			Ok(Outcome::NaturalEnd),
			Ok(Outcome::Stopped),
			Ok(Outcome::Stopped),
			Err(crate::error::Error::new(
				"The Player reported a playback error.",
			)),
		],
	);
}

#[tokio::test]
async fn player_monitor_finishes_a_keep_running_adapter_at_end_of_file() {
	let mut child = tokio::process::Command::new("sh")
		.args(["-c", "exec sleep 60"])
		.spawn()
		.unwrap();

	let progress = Arc::new(AtomicU64::new(754));
	let outcome = wait(
		&mut child,
		std::future::ready(Ok(Some("eof".into()))),
		progress,
	)
	.await
	.unwrap();

	assert_eq!(
		outcome,
		Report {
			outcome: Outcome::NaturalEnd,
			position: Position::from_seconds(754),
		}
	);
	assert!(child.try_wait().unwrap().is_some());
}

#[tokio::test]
async fn cancellation_requests_player_shutdown_over_its_private_ipc_socket() {
	use tokio::io::AsyncReadExt;

	let path = std::env::temp_dir().join(format!(
		"a365-player-test-{}.sock",
		uuid::Uuid::now_v7().simple()
	));
	let listener = tokio::net::UnixListener::bind(&path).unwrap();
	let request = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut request = String::new();
		stream.read_to_string(&mut request).await.unwrap();
		request
	});

	request_player_quit(&path).await.unwrap();
	assert_eq!(request.await.unwrap(), "{\"command\":[\"quit\"]}\n");
	tokio::fs::remove_file(path).await.unwrap();
}

#[tokio::test]
async fn player_monitor_loads_native_ass_after_media_is_ready() {
	let path = std::env::temp_dir().join(format!(
		"a365-player-test-{}.sock",
		uuid::Uuid::now_v7().simple()
	));
	let listener = tokio::net::UnixListener::bind(&path).unwrap();
	let subtitle = "http://127.0.0.1:43815/capability/subtitle";
	let player = tokio::spawn(async move {
		let (stream, _) = listener.accept().await.unwrap();
		let (read, mut write) = stream.into_split();
		let mut lines = BufReader::new(read).lines();
		let mut requests = Vec::new();
		for response in [
			json!({"request_id": 1, "error": "property unavailable"}),
			json!({"request_id": 2, "error": "success", "data": "media"}),
			json!({"request_id": 3, "error": "success"}),
			json!({"request_id": 100, "error": "success"}),
		] {
			let line = lines.next_line().await.unwrap().unwrap();
			requests.push(serde_json::from_str::<Value>(&line).unwrap());
			write
				.write_all(format!("{response}\n").as_bytes())
				.await
				.unwrap();
		}
		write
			.write_all(
				b"{\"event\":\"property-change\",\"name\":\"time-pos\",\"data\":754.8}\n{\"event\":\"end-file\",\"reason\":\"eof\"}\n",
			)
			.await
			.unwrap();
		requests
	});

	let progress = Arc::new(AtomicU64::new(0));
	let observed = tokio::time::timeout(
		Duration::from_secs(2),
		super::monitor(
			path.clone(),
			Some(subtitle.into()),
			Arc::clone(&progress),
		),
	)
	.await
	.unwrap();
	assert_eq!(observed, Ok(Some("eof".into())));
	assert_eq!(
		super::current_position(&progress),
		Position::from_seconds(754)
	);
	assert_eq!(
		player.await.unwrap(),
		vec![
			json!({"command": ["get_property", "path"], "request_id": 1}),
			json!({"command": ["get_property", "path"], "request_id": 2}),
			json!({"command": ["sub-add", subtitle, "select"], "request_id": 3}),
			json!({"command": ["observe_property", 1, "time-pos"], "request_id": 100}),
		]
	);
	tokio::fs::remove_file(path).await.unwrap();
}
