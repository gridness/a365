use pretty_assertions::assert_eq;

use super::{
	Outcome, PlayerKind, build_player_command, next_whole_episode,
	outcome_from_end_reason, request_player_quit, wait,
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
fn player_commands_receive_only_loopback_media_and_native_ass_urls() {
	let media = "http://127.0.0.1:43815/capability/media";
	let subtitle = "http://127.0.0.1:43815/capability/subtitle";
	let socket = "/tmp/a365-player.sock";

	assert_eq!(
		[
			build_player_command(
				PlayerKind::Iina,
				"iina-cli".into(),
				media,
				Some(subtitle),
				"Series — Episode 1",
				socket,
			),
			build_player_command(
				PlayerKind::Mpv,
				"mpv".into(),
				media,
				Some(subtitle),
				"Series — Episode 1",
				socket,
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
					format!("--mpv-sub-file={subtitle}"),
					media.into(),
				],
			},
			super::PlayerCommand {
				program: "mpv".into(),
				arguments: vec![
					format!("--input-ipc-server={socket}"),
					"--force-media-title=Series — Episode 1".into(),
					format!("--sub-file={subtitle}"),
					media.into(),
				],
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

	let outcome = wait(&mut child, std::future::ready(Some("eof".into())))
		.await
		.unwrap();

	assert_eq!(outcome, Outcome::NaturalEnd);
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
