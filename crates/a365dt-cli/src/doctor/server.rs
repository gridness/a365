use std::time::{Duration, Instant};

use reqwest::StatusCode;

use super::{milliseconds, report::Status};
use crate::community;

pub(super) const URL: &str =
	"https://anime365.ru/api/series/?limit=1&fields=id";
pub(super) const H365_URL: &str =
	"https://hentai365.ru/api/series/?limit=1&fields=id";
pub(super) const COMMUNITY_URL: &str =
	"https://smotret-anime.org/moments/index";
pub(super) const TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const LATENCY_WARNING: Duration = Duration::from_secs(1);

pub(super) struct Probe {
	pub status: Status,
	pub summary: String,
	pub http_status: Option<StatusCode>,
	pub latency: Duration,
	pub detail: Option<String>,
}

pub(super) async fn probe() -> Probe {
	probe_at(URL, ResponseContract::Reachable).await
}

pub(super) async fn probe_h365() -> Probe {
	probe_at(H365_URL, ResponseContract::Reachable).await
}

pub(super) async fn probe_community() -> Probe {
	probe_at(COMMUNITY_URL, ResponseContract::MomentsMarkup).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResponseContract {
	Reachable,
	MomentsMarkup,
}

async fn probe_at(url: &str, contract: ResponseContract) -> Probe {
	let started = Instant::now();
	let client = match reqwest::Client::builder()
		.https_only(true)
		.connect_timeout(TIMEOUT)
		.timeout(TIMEOUT)
		.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
		.build()
	{
		Ok(client) => client,
		Err(error) => {
			return Probe {
				status: Status::Error,
				summary: "Unavailable".into(),
				http_status: None,
				latency: started.elapsed(),
				detail: Some(error.to_string()),
			};
		}
	};
	match client.get(url).send().await {
		Ok(response) => {
			let http_status = response.status();
			let body = response.bytes().await;
			let latency = started.elapsed();
			if !http_status.is_success() {
				Probe {
					status: Status::Error,
					summary: format!("Unavailable (HTTP {http_status})"),
					http_status: Some(http_status),
					latency,
					detail: None,
				}
			} else if let Err(error) = body {
				Probe {
					status: Status::Error,
					summary: "Response could not be read".into(),
					http_status: Some(http_status),
					latency,
					detail: Some(error.to_string()),
				}
			} else if let Some(detail) = body
				.as_deref()
				.ok()
				.and_then(|body| contract_error(contract, body))
			{
				Probe {
					status: Status::Error,
					summary: "Markup contract changed".into(),
					http_status: Some(http_status),
					latency,
					detail: Some(detail),
				}
			} else {
				let status = if latency >= LATENCY_WARNING {
					Status::Warning
				} else {
					Status::Healthy
				};
				Probe {
					status,
					summary: format!(
						"Available · {}{}",
						milliseconds(latency.as_micros() as u64),
						if status == Status::Warning {
							" · elevated latency"
						} else {
							""
						}
					),
					http_status: Some(http_status),
					latency,
					detail: None,
				}
			}
		}
		Err(error) => Probe {
			status: Status::Error,
			summary: if error.is_timeout() {
				"Unavailable · timed out".into()
			} else {
				"Unavailable · request failed".into()
			},
			http_status: None,
			latency: started.elapsed(),
			detail: Some(error.without_url().to_string()),
		},
	}
}

pub(super) fn contract_error(
	contract: ResponseContract,
	body: &[u8],
) -> Option<String> {
	match contract {
		ResponseContract::Reachable => None,
		ResponseContract::MomentsMarkup => std::str::from_utf8(body)
			.map_err(|error| error.to_string())
			.and_then(|html| {
				community::validate_moments_markup(html)
					.map_err(|error| error.message().to_owned())
			})
			.err(),
	}
}
