use std::{convert::Infallible, net::Ipv4Addr};

use pretty_assertions::assert_eq;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
};

use super::LocalSubtitle;

#[tokio::test]
async fn fetched_subtitle_is_a_lifetime_scoped_ass_file() {
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
	let address = listener.local_addr().unwrap();
	let server = tokio::spawn(async move {
		let (mut stream, _) = listener.accept().await.unwrap();
		let mut request = [0; 1024];
		let _ = stream.read(&mut request).await.unwrap();
		stream
			.write_all(
				b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n[Script Info]",
			)
			.await
			.unwrap();
		Ok::<(), Infallible>(())
	});

	let subtitle = LocalSubtitle::fetch(&format!("http://{address}/subtitle"))
		.await
		.unwrap();
	server.await.unwrap().unwrap();
	let path = subtitle.path().to_owned();

	assert_eq!(
		(
			path.extension().and_then(|extension| extension.to_str()),
			tokio::fs::read(&path).await.unwrap(),
		),
		(Some("ass"), b"[Script Info]".to_vec()),
	);
	drop(subtitle);
	assert_eq!(path.exists(), false);
}
