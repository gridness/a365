use std::{collections::HashMap, sync::Arc};

use reqwest::{Client, Method, Response, header};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
	sync::watch,
	task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

use crate::{api::Anime365, error::Error, select::PlannedRelease};

const MAX_REQUEST_HEAD: usize = 32 * 1024;

pub(super) struct MediaProxy {
	pub(super) media_url: String,
	pub(super) subtitle_url: Option<String>,
	shutdown: watch::Sender<bool>,
	task: JoinHandle<()>,
}

#[derive(Clone)]
enum Upstream {
	Authenticated(Anime365),
	Public(Client),
}

impl MediaProxy {
	pub(super) async fn start(
		api: Anime365,
		release: &PlannedRelease,
	) -> Result<Self, Error> {
		Self::start_with(
			Upstream::Authenticated(api),
			release.media_url.clone(),
			release.subtitle_url.clone(),
		)
		.await
	}

	pub(super) async fn start_public(url: String) -> Result<Self, Error> {
		let client = Client::builder()
			.https_only(true)
			.connect_timeout(std::time::Duration::from_secs(10))
			.timeout(std::time::Duration::from_secs(30))
			.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
			.build()
			.map_err(|error| {
				Error::with_debug(
					"Could not initialize public Moment playback.",
					error,
				)
			})?;
		Self::start_with(Upstream::Public(client), url, None).await
	}

	async fn start_with(
		upstream: Upstream,
		media: String,
		subtitle: Option<String>,
	) -> Result<Self, Error> {
		let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
			.await
			.map_err(|error| {
				Error::with_debug(
					"Could not bind the local playback boundary.",
					error,
				)
			})?;
		let address = listener.local_addr().map_err(|error| {
			Error::with_debug(
				"Could not inspect the local playback boundary.",
				error,
			)
		})?;
		let capability = Uuid::now_v7().simple().to_string();
		let media_path = format!("/{capability}/media");
		let subtitle_path = format!("/{capability}/subtitle");
		let mut assets = HashMap::from([(media_path.clone(), media)]);
		if let Some(url) = &subtitle {
			assets.insert(subtitle_path.clone(), url.to_owned());
		}
		let assets = Arc::new(assets);
		let (shutdown, mut stopping) = watch::channel(false);
		let task = tokio::spawn(async move {
			let mut connections = JoinSet::new();
			loop {
				tokio::select! {
					accepted = listener.accept() => {
						let Ok((stream, _)) = accepted else {
							break;
						};
						let upstream = upstream.clone();
						let assets = Arc::clone(&assets);
						connections.spawn(async move {
							let _ = serve(stream, &upstream, &assets).await;
						});
					}
					completed = connections.join_next(), if !connections.is_empty() => {
						let _ = completed;
					}
					changed = stopping.changed() => {
						if changed.is_err() || *stopping.borrow() {
							break;
						}
					}
				}
			}
		});
		let origin = format!("http://{address}");
		Ok(Self {
			media_url: format!("{origin}{media_path}"),
			subtitle_url: subtitle
				.as_ref()
				.map(|_| format!("{origin}{subtitle_path}")),
			shutdown,
			task,
		})
	}
}

impl Drop for MediaProxy {
	fn drop(&mut self) {
		let _ = self.shutdown.send(true);
		self.task.abort();
	}
}

async fn serve(
	mut stream: TcpStream,
	upstream: &Upstream,
	assets: &HashMap<String, String>,
) -> Result<(), Error> {
	let request = read_request(&mut stream).await?;
	let Some(url) = assets.get(&request.path) else {
		return write_empty(&mut stream, 404, "Not Found").await;
	};
	if !matches!(request.method, Method::GET | Method::HEAD) {
		return write_empty(&mut stream, 405, "Method Not Allowed").await;
	}
	let response = upstream_response(upstream, &request, url).await?;
	let status = response.status();
	let mut head = format!(
		"HTTP/1.1 {} {}\r\nConnection: close\r\n",
		status.as_u16(),
		status.canonical_reason().unwrap_or("Response")
	);
	for name in [
		header::CONTENT_TYPE,
		header::CONTENT_LENGTH,
		header::CONTENT_RANGE,
		header::ACCEPT_RANGES,
		header::ETAG,
		header::LAST_MODIFIED,
	] {
		if let Some(value) = response.headers().get(&name)
			&& let Ok(value) = value.to_str()
		{
			head.push_str(name.as_str());
			head.push_str(": ");
			head.push_str(value);
			head.push_str("\r\n");
		}
	}
	head.push_str("\r\n");
	stream
		.write_all(head.as_bytes())
		.await
		.map_err(proxy_error)?;
	if request.method == Method::HEAD {
		return Ok(());
	}
	let mut response = response;
	while let Some(chunk) = response.chunk().await.map_err(|error| {
		Error::with_debug(
			"The playback boundary lost its upstream response.",
			error.without_url(),
		)
	})? {
		stream.write_all(&chunk).await.map_err(proxy_error)?;
	}
	Ok(())
}

async fn upstream_response(
	upstream: &Upstream,
	request: &Request,
	url: &str,
) -> Result<Response, Error> {
	let range = request.headers.get("range").map(String::as_str);
	let if_range = request.headers.get("if-range").map(String::as_str);
	match upstream {
		Upstream::Authenticated(api) => {
			api.proxy_asset(request.method.clone(), url, range, if_range)
				.await
		}
		Upstream::Public(client) => {
			let mut upstream = client.request(request.method.clone(), url);
			if let Some(range) = range {
				upstream = upstream.header(header::RANGE, range);
			}
			if let Some(if_range) = if_range {
				upstream = upstream.header(header::IF_RANGE, if_range);
			}
			upstream.send().await.map_err(|error| {
				Error::with_debug(
					"Could not load public Moment media.",
					error.without_url(),
				)
			})
		}
	}
}

struct Request {
	method: Method,
	path: String,
	headers: HashMap<String, String>,
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, Error> {
	let mut bytes = Vec::new();
	let mut chunk = [0; 2048];
	loop {
		let read = stream.read(&mut chunk).await.map_err(proxy_error)?;
		if read == 0 {
			return Err(Error::new(
				"The Player closed an incomplete playback request.",
			));
		}
		bytes.extend_from_slice(&chunk[..read]);
		if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
			break;
		}
		if bytes.len() > MAX_REQUEST_HEAD {
			return Err(Error::new("The Player sent an oversized request."));
		}
	}
	let text = std::str::from_utf8(&bytes).map_err(|error| {
		Error::with_debug("The Player sent an unreadable request.", error)
	})?;
	let mut lines = text.split("\r\n");
	let request_line = lines.next().unwrap_or_default();
	let mut parts = request_line.split_whitespace();
	let method = parts
		.next()
		.and_then(|method| Method::from_bytes(method.as_bytes()).ok())
		.ok_or_else(|| Error::new("The Player sent an invalid HTTP method."))?;
	let path = parts
		.next()
		.ok_or_else(|| Error::new("The Player omitted the media path."))?
		.split('?')
		.next()
		.unwrap_or_default()
		.to_owned();
	let headers = lines
		.take_while(|line| !line.is_empty())
		.filter_map(|line| line.split_once(':'))
		.map(|(name, value)| {
			(name.trim().to_ascii_lowercase(), value.trim().to_owned())
		})
		.collect();
	Ok(Request {
		method,
		path,
		headers,
	})
}

async fn write_empty(
	stream: &mut TcpStream,
	status: u16,
	reason: &str,
) -> Result<(), Error> {
	stream
		.write_all(
			format!(
				"HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
			)
			.as_bytes(),
		)
		.await
		.map_err(proxy_error)
}

fn proxy_error(error: std::io::Error) -> Error {
	Error::with_debug("The local playback boundary failed.", error)
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
