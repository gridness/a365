use std::net::Ipv4Addr;

use pretty_assertions::assert_eq;
use reqwest::Method;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
};

use super::{MediaProxy, read_request};
use crate::{
	api::{Anime365, Episode, Translation},
	content::ContentSource,
	select::PlannedRelease,
};

fn release() -> PlannedRelease {
	PlannedRelease {
		episode: Episode {
			source: ContentSource::Anime365,
			id: 7,
			episode_int: "1".into(),
			episode_full: "Episode 1".into(),
		},
		translation: Translation {
			source: ContentSource::Anime365,
			id: 8,
			episode_id: 7,
			kind: "sub".into(),
			language: "ru".into(),
			authors_summary: "Team".into(),
		},
		height: 1080,
		media_url: "https://smotret-anime.org/video.mp4?access_token=secret"
			.into(),
		subtitle_url: Some(
			"https://smotret-anime.org/subtitle.ass?access_token=secret".into(),
		),
	}
}

#[tokio::test]
async fn player_urls_are_random_loopback_capabilities_without_upstream_data() {
	let api = Anime365::new("secret".into(), Default::default()).unwrap();
	let first = MediaProxy::start(api.clone(), &release()).await.unwrap();
	let second = MediaProxy::start(api, &release()).await.unwrap();

	for url in [
		&first.media_url,
		first.subtitle_url.as_ref().unwrap(),
		&second.media_url,
	] {
		assert!(url.starts_with("http://127.0.0.1:"));
		assert!(!url.contains("secret") && !url.contains("smotret-anime.org"));
	}
	assert_ne!(first.media_url, second.media_url);
}

#[tokio::test]
async fn request_boundary_preserves_only_method_path_and_range_controls() {
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
	let address = listener.local_addr().unwrap();
	let client = tokio::spawn(async move {
		let mut stream = TcpStream::connect(address).await.unwrap();
		stream
			.write_all(
				b"HEAD /capability/media?ignored=yes HTTP/1.1\r\nRange: bytes=10-20\r\nIf-Range: validator\r\nAuthorization: secret\r\n\r\n",
			)
			.await
			.unwrap();
	});
	let (mut stream, _) = listener.accept().await.unwrap();
	let request = read_request(&mut stream).await.unwrap();
	client.await.unwrap();

	assert_eq!(
		(
			request.method,
			request.path,
			request.headers.get("range").cloned(),
			request.headers.get("if-range").cloned(),
		),
		(
			Method::HEAD,
			"/capability/media".into(),
			Some("bytes=10-20".into()),
			Some("validator".into()),
		)
	);
}

#[tokio::test]
async fn unknown_capability_is_rejected_before_contacting_upstream() {
	let proxy = MediaProxy::start(
		Anime365::new("secret".into(), Default::default()).unwrap(),
		&release(),
	)
	.await
	.unwrap();
	let url = reqwest::Url::parse(&proxy.media_url).unwrap();
	let mut stream =
		TcpStream::connect((Ipv4Addr::LOCALHOST, url.port().unwrap()))
			.await
			.unwrap();
	stream
		.write_all(b"GET /wrong HTTP/1.1\r\nHost: localhost\r\n\r\n")
		.await
		.unwrap();
	let mut response = String::new();
	stream.read_to_string(&mut response).await.unwrap();

	assert!(response.starts_with("HTTP/1.1 404 Not Found"));
}
