use pretty_assertions::assert_eq;

use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
};

use super::{
	CALLBACK_ADDRESS, Relay, authorization_url, relay_page,
	validate_relay_nonce, wait_for_grant, wait_for_grant_until,
};

#[test]
fn constructs_implicit_authorization_without_overriding_registered_callback() {
	let url = authorization_url("365").unwrap();
	let parameters = url.query_pairs().collect::<Vec<_>>();

	assert_eq!(
		(url.scheme(), url.host_str(), url.path(), parameters),
		(
			"https",
			Some("anilist.co"),
			"/api/v2/oauth/authorize",
			vec![
				("client_id".into(), "365".into()),
				("response_type".into(), "token".into()),
			],
		)
	);
}

#[test]
fn callback_is_fixed_to_loopback_and_fragment_relay_never_displays_a_token() {
	let address = CALLBACK_ADDRESS.parse::<std::net::SocketAddr>().unwrap();
	let page = relay_page("local-nonce");

	assert!(address.ip().is_loopback());
	assert_eq!(address.port(), 43_815);
	assert!(
		page.contains("location.hash") && page.contains("history.replaceState")
	);
	assert!(page.contains("relay_nonce:\"local-nonce\""));
	assert!(!page.contains("p.get(\"state\")"));
	assert!(!page.contains("innerHTML") && !page.contains("console."));
}

#[test]
fn local_relay_nonce_rejects_a_stale_callback_page() {
	let relay = |nonce: &str| Relay {
		access_token: None,
		relay_nonce: Some(nonce.to_owned()),
		error: None,
		expires_in: None,
	};

	assert_eq!(
		[
			validate_relay_nonce(&relay("current"), "current"),
			validate_relay_nonce(&relay("stale"), "current"),
		],
		[
			Ok(()),
			Err(crate::error::Error::new(
				"The AniList callback did not match this login attempt. No token was saved.",
			)),
		],
	);
}

#[tokio::test]
async fn loopback_listener_relays_the_fragment_without_echoing_the_token() {
	let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
		.await
		.unwrap();
	let address = listener.local_addr().unwrap();
	let browser = async move {
		let mut callback = TcpStream::connect(address).await.unwrap();
		callback
			.write_all(
				b"GET /anilist/callback HTTP/1.1\r\nHost: localhost\r\n\r\n",
			)
			.await
			.unwrap();
		let mut page = Vec::new();
		callback.read_to_end(&mut page).await.unwrap();
		let page = String::from_utf8(page).unwrap();
		assert!(
			page.contains("relay-nonce") && !page.contains("fixture-token")
		);

		let body = br#"{"access_token":"fixture-token","relay_nonce":"relay-nonce","error":null,"expires_in":31536000}"#;
		let mut relay = TcpStream::connect(address).await.unwrap();
		relay
			.write_all(
				format!(
					"POST /anilist/token HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
					body.len()
				)
				.as_bytes(),
			)
			.await
			.unwrap();
		relay.write_all(body).await.unwrap();
	};
	let (grant, ()) =
		tokio::join!(wait_for_grant(&listener, "relay-nonce"), browser);
	let grant = grant.unwrap();

	assert_eq!(
		(grant.token, grant.expires_in),
		("fixture-token".into(), Some(31_536_000))
	);
}

#[tokio::test]
async fn pending_login_times_out_with_an_actionable_retry() {
	let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
		.await
		.unwrap();

	let Err(error) = wait_for_grant_until(
		&listener,
		"unused",
		std::time::Duration::from_millis(1),
	)
	.await
	else {
		panic!("expected the pending login to time out");
	};

	assert_eq!(
		error,
		crate::error::Error::new(
			"AniList login timed out after five minutes. Retry from the AniList tab.",
		),
	);
}

#[tokio::test]
async fn browser_denial_cancels_the_pending_login_without_a_token() {
	let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
		.await
		.unwrap();
	let address = listener.local_addr().unwrap();
	let browser = async move {
		let body = br#"{"access_token":null,"relay_nonce":"relay-nonce","error":"access_denied","expires_in":null}"#;
		let mut relay = TcpStream::connect(address).await.unwrap();
		relay
			.write_all(
				format!(
					"POST /anilist/token HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
					body.len()
				)
				.as_bytes(),
			)
			.await
			.unwrap();
		relay.write_all(body).await.unwrap();
	};
	let (grant, ()) =
		tokio::join!(wait_for_grant(&listener, "relay-nonce"), browser);

	let Err(error) = grant else {
		panic!("expected browser denial to cancel the login");
	};
	assert_eq!(
		error,
		crate::error::Error::new(
			"AniList login was denied or cancelled in the browser.",
		),
	);
}
