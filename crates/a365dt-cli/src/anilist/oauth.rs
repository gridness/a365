use std::{io, time::Duration};

use reqwest::Url;
use serde::Deserialize;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
	time::timeout,
};
use uuid::Uuid;

use crate::{auth, error::Error, ui};

const CALLBACK_ADDRESS: &str = "127.0.0.1:43815";
const CALLBACK_PATH: &str = "/anilist/callback";
const TOKEN_PATH: &str = "/anilist/token";
const AUTHORIZE: &str = "https://anilist.co/api/v2/oauth/authorize";
const DEFAULT_CLIENT_ID: &str = "49510";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_REQUEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy)]
enum Presentation {
	Line,
	Tui,
}

pub(super) struct Grant {
	pub(super) token: String,
	pub(super) expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct Relay {
	access_token: Option<String>,
	relay_nonce: Option<String>,
	error: Option<String>,
	expires_in: Option<u64>,
}

struct Request {
	method: String,
	path: String,
	body: Vec<u8>,
}

pub(super) async fn login() -> Result<Grant, Error> {
	login_with(Presentation::Line).await
}

pub(super) async fn login_in_tui() -> Result<Grant, Error> {
	login_with(Presentation::Tui).await
}

async fn login_with(presentation: Presentation) -> Result<Grant, Error> {
	let client_id = client_id();
	let listener = bind().await?;
	let relay_nonce = Uuid::now_v7().to_string();
	let url = authorization_url(&client_id)?;
	if matches!(presentation, Presentation::Line) {
		ui::note("Opening AniList authorization in your browser…");
	}
	if !auth::open_browser(url.as_str()) {
		match presentation {
			Presentation::Line => {
				ui::warning(format!("Could not open the browser. Visit {url}"));
			}
			Presentation::Tui => {
				return Err(Error::new(format!(
					"Could not open the browser for AniList authorization. Visit {url} and retry from the AniList tab."
				)));
			}
		}
	}
	if matches!(presentation, Presentation::Line) {
		ui::note("Waiting for AniList approval…");
	}
	wait_for_grant_until(&listener, &relay_nonce, LOGIN_TIMEOUT).await
}

async fn wait_for_grant_until(
	listener: &TcpListener,
	expected_relay_nonce: &str,
	duration: Duration,
) -> Result<Grant, Error> {
	timeout(duration, wait_for_grant(listener, expected_relay_nonce))
		.await
		.map_err(|_| login_timeout_error())?
}

fn login_timeout_error() -> Error {
	Error::new(
		"AniList login timed out after five minutes. Retry from the AniList tab.",
	)
}

fn client_id() -> String {
	std::env::var("ANILIST_CLIENT_ID")
		.ok()
		.or_else(|| option_env!("ANILIST_CLIENT_ID").map(str::to_owned))
		.filter(|value| !value.trim().is_empty())
		.map(|value| value.trim().to_owned())
		.unwrap_or_else(|| DEFAULT_CLIENT_ID.to_owned())
}

async fn bind() -> Result<TcpListener, Error> {
	TcpListener::bind(CALLBACK_ADDRESS).await.map_err(|error| {
		let message = if error.kind() == io::ErrorKind::AddrInUse {
			"AniList login cannot start because local callback port 43815 is already in use. Close the other process and retry."
		} else {
			"AniList login could not bind its loopback callback."
		};
		Error::with_debug(message, error)
	})
}

fn authorization_url(client_id: &str) -> Result<Url, Error> {
	let mut url = Url::parse(AUTHORIZE).map_err(|error| {
		Error::with_debug("Could not construct the AniList login URL.", error)
	})?;
	url.query_pairs_mut()
		.append_pair("client_id", client_id)
		.append_pair("response_type", "token");
	Ok(url)
}

async fn wait_for_grant(
	listener: &TcpListener,
	expected_relay_nonce: &str,
) -> Result<Grant, Error> {
	loop {
		let (mut stream, peer) = listener.accept().await.map_err(|error| {
			Error::with_debug("AniList callback could not be accepted.", error)
		})?;
		if !peer.ip().is_loopback() {
			continue;
		}
		let request = match read_request(&mut stream).await {
			Ok(request) => request,
			Err(error) => {
				respond(
					&mut stream,
					"400 Bad Request",
					"text/plain",
					b"Invalid request",
				)
				.await;
				return Err(error);
			}
		};
		match (request.method.as_str(), request.path.as_str()) {
			("GET", CALLBACK_PATH) => {
				let page = relay_page(expected_relay_nonce);
				respond(
					&mut stream,
					"200 OK",
					"text/html; charset=utf-8",
					page.as_bytes(),
				)
				.await;
			}
			("POST", TOKEN_PATH) => {
				let relay = serde_json::from_slice::<Relay>(&request.body)
					.map_err(|error| {
						Error::with_debug(
							"AniList returned an unreadable browser callback.",
							error,
						)
					})?;
				if let Err(error) =
					validate_relay_nonce(&relay, expected_relay_nonce)
				{
					respond(
						&mut stream,
						"403 Forbidden",
						"text/plain",
						b"Invalid relay nonce",
					)
					.await;
					return Err(error);
				}
				if relay.error.is_some() {
					respond(&mut stream, "204 No Content", "text/plain", b"")
						.await;
					return Err(Error::new(
						"AniList login was denied or cancelled in the browser.",
					));
				}
				let token = relay
					.access_token
					.filter(|token| !token.trim().is_empty())
					.ok_or_else(|| {
						Error::new("AniList returned no access token.")
					})?;
				respond(&mut stream, "204 No Content", "text/plain", b"").await;
				return Ok(Grant {
					token,
					expires_in: relay.expires_in,
				});
			}
			_ => {
				respond(
					&mut stream,
					"404 Not Found",
					"text/plain",
					b"Not found",
				)
				.await;
			}
		}
	}
}

fn validate_relay_nonce(
	relay: &Relay,
	expected_relay_nonce: &str,
) -> Result<(), Error> {
	if relay.relay_nonce.as_deref() != Some(expected_relay_nonce) {
		return Err(Error::new(
			"The AniList callback did not match this login attempt. No token was saved.",
		));
	}
	Ok(())
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, Error> {
	let mut bytes = Vec::new();
	let header_end = loop {
		if bytes.len() >= MAX_REQUEST_BYTES {
			return Err(Error::new("AniList callback request was too large."));
		}
		let mut chunk = [0; 1024];
		let read = stream.read(&mut chunk).await.map_err(|error| {
			Error::with_debug("Could not read the AniList callback.", error)
		})?;
		if read == 0 {
			return Err(Error::new("AniList callback ended unexpectedly."));
		}
		bytes.extend_from_slice(&chunk[..read]);
		if let Some(position) =
			bytes.windows(4).position(|window| window == b"\r\n\r\n")
		{
			break position + 4;
		}
	};
	let headers =
		std::str::from_utf8(&bytes[..header_end]).map_err(|error| {
			Error::with_debug(
				"AniList callback headers were unreadable.",
				error,
			)
		})?;
	let mut lines = headers.lines();
	let mut request_line = lines
		.next()
		.ok_or_else(|| Error::new("AniList callback was empty."))?
		.split_whitespace();
	let method = request_line.next().unwrap_or_default().to_owned();
	let path = request_line
		.next()
		.unwrap_or_default()
		.split('?')
		.next()
		.unwrap_or_default()
		.to_owned();
	let content_length = lines
		.find_map(|line| {
			line.split_once(':').and_then(|(name, value)| {
				name.eq_ignore_ascii_case("content-length")
					.then(|| value.trim().parse::<usize>().ok())
					.flatten()
			})
		})
		.unwrap_or_default();
	if header_end.saturating_add(content_length) > MAX_REQUEST_BYTES {
		return Err(Error::new("AniList callback request was too large."));
	}
	while bytes.len() < header_end + content_length {
		let mut chunk = [0; 1024];
		let read = stream.read(&mut chunk).await.map_err(|error| {
			Error::with_debug("Could not read the AniList callback.", error)
		})?;
		if read == 0 {
			return Err(Error::new("AniList callback ended unexpectedly."));
		}
		bytes.extend_from_slice(&chunk[..read]);
	}
	Ok(Request {
		method,
		path,
		body: bytes[header_end..header_end + content_length].to_vec(),
	})
}

async fn respond(
	stream: &mut TcpStream,
	status: &str,
	content_type: &str,
	body: &[u8],
) {
	let headers = format!(
		"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'\r\nConnection: close\r\n\r\n",
		body.len()
	);
	let _ = stream.write_all(headers.as_bytes()).await;
	let _ = stream.write_all(body).await;
}

fn relay_page(relay_nonce: &str) -> String {
	r#"<!doctype html><meta charset="utf-8"><meta name="referrer" content="no-referrer"><title>a365 · AniList</title><style>body{font:16px system-ui;margin:4rem;max-width:42rem}p{color:#445}</style><h1 id="status">Finishing AniList login…</h1><p>You can close this tab when a365 confirms the connection.</p><script>(()=>{const p=new URLSearchParams(location.hash.slice(1));history.replaceState(null,"",location.pathname);const body={access_token:p.get("access_token"),relay_nonce:"__RELAY_NONCE__",error:p.get("error"),expires_in:Number(p.get("expires_in"))||null};fetch("/anilist/token",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(body)}).then(r=>{if(!r.ok)throw 0;document.getElementById("status").textContent=body.error?"AniList login was cancelled":"AniList connected to a365"}).catch(()=>document.getElementById("status").textContent="Return to a365 and retry login")})()</script>"#
		.replace("__RELAY_NONCE__", relay_nonce)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
