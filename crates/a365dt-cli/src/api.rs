use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Response, Url, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
	content::{ContentSource, SeriesKey},
	error::Error,
	telemetry::{Operation, Recorder},
};

const ANIME365_ASSET_ORIGIN: &str = "https://smotret-anime.org";
const ANIME365_API: &str = "https://anime365.ru/api";
const H365_ASSET_ORIGIN: &str = "https://hentai365.ru";
const H365_API: &str = "https://hentai365.ru/api";
const SERIES_FIELDS: &str =
	"id,title,year,typeTitle,numberOfEpisodes,myAnimeListId,anilistId";
pub const SERIES_PAGE_SIZE: usize = 1_000;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AccessFailure {
	Denied(Error),
	Unavailable(Error),
}

impl AccessFailure {
	pub(crate) fn into_error(self) -> Error {
		match self {
			Self::Denied(error) | Self::Unavailable(error) => error,
		}
	}
}

#[derive(Clone)]
pub struct Anime365 {
	http: Client,
	token: String,
	telemetry: Recorder,
	source: ContentSource,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
	#[serde(default)]
	pub source: ContentSource,
	pub id: u64,
	pub title: String,
	pub year: Option<u16>,
	pub type_title: Option<String>,
	pub number_of_episodes: Option<u32>,
	#[serde(default)]
	pub my_anime_list_id: Option<u64>,
	#[serde(default)]
	pub anilist_id: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub poster_url_small: Option<String>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub episodes: Vec<Episode>,
}

impl Series {
	pub(crate) const fn key(&self) -> SeriesKey {
		SeriesKey::new(self.source, self.id)
	}
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
	#[serde(default)]
	pub source: ContentSource,
	pub id: u64,
	pub episode_int: String,
	pub episode_full: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Translation {
	#[serde(default)]
	pub source: ContentSource,
	pub id: u64,
	pub episode_id: u64,
	#[serde(rename = "typeKind")]
	pub kind: String,
	#[serde(rename = "typeLang")]
	pub language: String,
	#[serde(default)]
	pub authors_summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Embed {
	#[serde(default)]
	pub download: Vec<MediaOption>,
	pub subtitles_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct MediaOption {
	pub height: u16,
	pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Profile {
	pub is_logined: bool,
	pub id: Option<u64>,
	pub name: Option<String>,
	#[serde(default)]
	pub is_premium: bool,
	pub premium_until: Option<String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct Envelope<T> {
	data: Option<T>,
	error: Option<ApiError>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct ApiError {
	code: u16,
	message: String,
}

impl Anime365 {
	pub fn new(token: String, telemetry: Recorder) -> Result<Self> {
		Self::for_source(ContentSource::Anime365, token, telemetry)
	}

	pub fn h365(token: String, telemetry: Recorder) -> Result<Self> {
		Self::for_source(ContentSource::H365, token, telemetry)
	}

	fn for_source(
		source: ContentSource,
		token: String,
		telemetry: Recorder,
	) -> Result<Self> {
		let http = Client::builder()
			.https_only(true)
			.connect_timeout(Duration::from_secs(30))
			.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
			.build()
			.map_err(|error| {
				request_error(
					"Could not initialize the secure HTTP client.",
					error,
				)
			})?;
		Ok(Self {
			http,
			token,
			telemetry,
			source,
		})
	}

	pub(crate) const fn source(&self) -> ContentSource {
		self.source
	}

	pub async fn validate(&self) -> Result<()> {
		self.validate_access()
			.await
			.map_err(AccessFailure::into_error)
	}

	pub(crate) async fn validate_access(
		&self,
	) -> std::result::Result<(), AccessFailure> {
		let request = self
			.http
			.get(format!("{}/me", api_origin(self.source)))
			.query(&[("access_token", &self.token)])
			.timeout(Duration::from_secs(30));
		let _measurement = self.telemetry.measure(Operation::ApiValidate);
		let response = request.send().await.map_err(|error| {
			AccessFailure::Unavailable(request_error(
				&format!("Could not reach {}.", self.source),
				error,
			))
		})?;
		let status = response.status();
		let body: Envelope<serde::de::IgnoredAny> =
			response.json().await.map_err(|error| {
				AccessFailure::Unavailable(request_error(
					&format!(
						"{} returned an unreadable access response.",
						self.source
					),
					error,
				))
			})?;
		if status == reqwest::StatusCode::UNAUTHORIZED
			|| status == reqwest::StatusCode::FORBIDDEN
			|| body
				.error
				.as_ref()
				.is_some_and(|error| matches!(error.code, 401 | 403))
		{
			return Err(AccessFailure::Denied(Error::new(format!(
				"{} denied access for the current Anime365 token.",
				self.source
			))));
		}
		if let Some(error) = body.error {
			return Err(AccessFailure::Unavailable(Error::new(format!(
				"{} error {}: {}",
				self.source, error.code, error.message
			))));
		}
		if !status.is_success() || body.data.is_none() {
			return Err(AccessFailure::Unavailable(Error::new(format!(
				"{} could not validate access (HTTP {status}).",
				self.source
			))));
		}
		Ok(())
	}

	pub async fn search(&self, query: &str) -> Result<Vec<Series>> {
		self.get(
			"/series/",
			&[
				("query", query.to_owned()),
				("limit", "10".into()),
				("fields", SERIES_FIELDS.into()),
			],
			false,
			Operation::ApiSearch,
		)
		.await
		.map(|series: Vec<Series>| self.source_series(series))
	}

	pub async fn series(&self, id: u64) -> Result<Option<Series>> {
		self.get_optional(
			&format!("/series/{id}"),
			&[],
			false,
			Operation::ApiSeries,
		)
		.await
		.map(|series: Option<Series>| {
			series.map(|series| self.source_series_item(series))
		})
	}

	pub async fn series_page(&self, offset: usize) -> Result<Vec<Series>> {
		self.get(
			"/series/",
			&[
				("limit", SERIES_PAGE_SIZE.to_string()),
				("offset", offset.to_string()),
				("fields", SERIES_FIELDS.into()),
			],
			false,
			Operation::ApiSeriesPage,
		)
		.await
		.map(|series: Vec<Series>| self.source_series(series))
	}

	pub async fn translations(
		&self,
		series_id: u64,
	) -> Result<Vec<Translation>> {
		let mut translations = Vec::new();
		loop {
			let page: Vec<Translation> = self
				.get::<Vec<Translation>>(
					"/translations/",
					&[
						("seriesId", series_id.to_string()),
						("limit", "1000".into()),
						("offset", translations.len().to_string()),
						(
							"fields",
							"id,episodeId,typeKind,typeLang,authorsSummary"
								.into(),
						),
					],
					false,
					Operation::ApiTranslations,
				)
				.await?;
			let page = source_translations(self.source, page);
			let done = page.len() < 1000;
			translations.extend(page);
			if done {
				return Ok(translations);
			}
			if translations.len() >= 100_000 {
				return Err("Anime365 returned too many translations.".into());
			}
		}
	}

	pub(crate) async fn profile(&self) -> Result<Profile> {
		self.get("/me", &[], true, Operation::ApiValidate).await
	}

	pub async fn embed(&self, translation_id: u64) -> Result<Embed> {
		self.get(
			&format!("/translations/embed/{translation_id}"),
			&[],
			true,
			Operation::ApiEmbed,
		)
		.await
	}

	pub async fn asset(&self, method: Method, url: &str) -> Result<Response> {
		let operation = if method == Method::HEAD {
			Operation::AssetHead
		} else {
			Operation::AssetGet
		};
		let request = self.asset_request(method, url)?;
		let _measurement = self.telemetry.measure(operation);
		send_asset(request, self.source).await
	}

	pub async fn asset_from(
		&self,
		url: &str,
		start: u64,
		validator: &str,
	) -> Result<Response> {
		let request = self
			.asset_request(Method::GET, url)?
			.header(header::RANGE, format!("bytes={start}-"))
			.header(header::IF_RANGE, validator);
		let _measurement = self.telemetry.measure(Operation::AssetResume);
		send_asset(request, self.source).await
	}

	pub(crate) async fn proxy_asset(
		&self,
		method: Method,
		url: &str,
		range: Option<&str>,
		if_range: Option<&str>,
	) -> Result<Response> {
		let mut request = self.asset_request(method, url)?;
		if let Some(range) = range {
			request = request.header(header::RANGE, range);
		}
		if let Some(if_range) = if_range {
			request = request.header(header::IF_RANGE, if_range);
		}
		let _measurement = self.telemetry.measure(Operation::AssetGet);
		send_asset(request, self.source).await
	}

	fn asset_request(
		&self,
		method: Method,
		url: &str,
	) -> Result<RequestBuilder> {
		let url = normalize_url(self.source, url)?;
		let mut request = self.http.request(method, url.clone());
		if is_official(&url) {
			request = request.query(&[("access_token", &self.token)]);
		}
		Ok(request)
	}

	async fn get<T: DeserializeOwned>(
		&self,
		path: &str,
		query: &[(&str, String)],
		authenticated: bool,
		operation: Operation,
	) -> Result<T> {
		self.get_optional(path, query, authenticated, operation)
			.await?
			.ok_or_else(|| {
				Error::new("Anime365 did not return the requested API data.")
			})
	}

	async fn get_optional<T: DeserializeOwned>(
		&self,
		path: &str,
		query: &[(&str, String)],
		authenticated: bool,
		operation: Operation,
	) -> Result<Option<T>> {
		let mut request = self
			.http
			.get(format!("{}{path}", api_origin(self.source)))
			.query(query)
			.timeout(Duration::from_secs(30));
		if authenticated {
			request = request.query(&[("access_token", &self.token)]);
		}
		let _measurement = self.telemetry.measure(operation);
		let response = request.send().await.map_err(|error| {
			request_error(
				&format!("The request to the {} API failed.", self.source),
				error,
			)
		})?;
		let status = response.status();
		let body: Envelope<T> = response.json().await.map_err(|error| {
			request_error(
				&format!(
					"{} returned a response a365 could not read.",
					self.source
				),
				error,
			)
		})?;
		if status == reqwest::StatusCode::NOT_FOUND
			|| body.error.as_ref().is_some_and(|error| error.code == 404)
		{
			return Ok(None);
		}
		if let Some(error) = body.error {
			return Err(Error::new(format!(
				"{} error {}: {}",
				self.source, error.code, error.message
			)));
		}
		if !status.is_success() {
			return Err(Error::new(format!(
				"{} rejected the API request (HTTP {status}).",
				self.source
			)));
		}
		Ok(body.data)
	}

	fn source_series(&self, series: Vec<Series>) -> Vec<Series> {
		series
			.into_iter()
			.map(|series| self.source_series_item(series))
			.collect()
	}

	fn source_series_item(&self, mut series: Series) -> Series {
		series.source = self.source;
		for episode in &mut series.episodes {
			episode.source = self.source;
		}
		series
	}
}

fn source_translations(
	source: ContentSource,
	mut translations: Vec<Translation>,
) -> Vec<Translation> {
	for translation in &mut translations {
		translation.source = source;
	}
	translations
}

pub fn series_key_from_url(input: &str) -> Option<SeriesKey> {
	let url = Url::parse(input).ok()?;
	let source = source_from_host(url.host_str()?)?;
	let parts: Vec<_> = url
		.path_segments()?
		.filter(|part| !part.is_empty())
		.collect();
	let id = (parts.len() == 2 && parts[0] == "catalog")
		.then(|| parts[1].rsplit('-').next()?.parse().ok())??;
	Some(SeriesKey::new(source, id))
}

fn normalize_url(source: ContentSource, input: &str) -> Result<Url> {
	let mut url = Url::parse(input)
		.or_else(|_| {
			Url::parse(asset_origin(source)).and_then(|base| base.join(input))
		})
		.map_err(|error| {
			Error::with_debug("Anime365 returned an invalid media URL.", error)
		})?;
	if matches!(url.host_str(), Some("anime365.ru" | "www.anime365.ru"))
		&& url.path().starts_with("/posters/")
	{
		url.set_host(Some("smotret-anime.org")).map_err(|error| {
			Error::with_debug("Anime365 returned an invalid poster URL.", error)
		})?;
	}
	Ok(url)
}

fn is_official(url: &Url) -> bool {
	url.host_str().and_then(source_from_host).is_some()
}

fn source_from_host(host: &str) -> Option<ContentSource> {
	match host {
		"anime365.ru"
		| "www.anime365.ru"
		| "smotret-anime.org"
		| "www.smotret-anime.org"
		| "smotret-anime.app"
		| "www.smotret-anime.app" => Some(ContentSource::Anime365),
		"hentai365.ru" | "www.hentai365.ru" => Some(ContentSource::H365),
		_ => None,
	}
}

const fn api_origin(source: ContentSource) -> &'static str {
	match source {
		ContentSource::Anime365 => ANIME365_API,
		ContentSource::H365 => H365_API,
	}
}

const fn asset_origin(source: ContentSource) -> &'static str {
	match source {
		ContentSource::Anime365 => ANIME365_ASSET_ORIGIN,
		ContentSource::H365 => H365_ASSET_ORIGIN,
	}
}

fn request_error(message: &str, error: reqwest::Error) -> Error {
	Error::with_debug(message, error.without_url())
}

async fn send_asset(
	request: RequestBuilder,
	source: ContentSource,
) -> Result<Response> {
	request.send().await.map_err(|error| {
		request_error(
			&format!("The request to the {source} media server failed."),
			error,
		)
	})
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
