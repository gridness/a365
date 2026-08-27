use std::time::Duration;

use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

use crate::{api::Anime365, error::Error, preferences::AdultContent};

#[path = "community/classification.rs"]
mod classification;

use classification::classify;

const ORIGIN: &str = "https://smotret-anime.org";

#[derive(Clone)]
pub(crate) struct CommunityClient {
	http: Client,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MomentPage {
	pub moments: Vec<Moment>,
	pub categories: Vec<MomentCategory>,
	pub next_page: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Moment {
	pub id: u64,
	pub title: String,
	pub duration: String,
	pub thumbnail_url: String,
	pub series_id: Option<u64>,
	pub episode: Option<String>,
	pub author: Option<String>,
	pub age_or_date: Option<String>,
	pub views: Option<u64>,
	pub is_adult: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MomentCategory {
	pub label: String,
	pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MomentMedia {
	pub url: String,
	pub height: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProfileEnrichment {
	pub avatar_url: Option<String>,
	pub lists: Vec<PublicListProgress>,
	pub moments: Vec<Moment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicListProgress {
	pub status: PublicListStatus,
	pub title: String,
	pub progress: Option<String>,
	pub score: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicListStatus {
	Watching,
	Planned,
	Completed,
	Paused,
	Dropped,
}

#[derive(Deserialize)]
struct MomentSource {
	height: u16,
	#[serde(default)]
	urls: Vec<String>,
}

#[derive(Clone, Copy)]
enum MomentOrdering {
	Recent,
	Popular,
}

impl CommunityClient {
	pub(crate) fn new() -> Result<Self, Error> {
		let http = Client::builder()
			.https_only(true)
			.connect_timeout(Duration::from_secs(10))
			.timeout(Duration::from_secs(20))
			.user_agent(concat!("a365/", env!("CARGO_PKG_VERSION")))
			.build()
			.map_err(|error| {
				Error::with_debug(
					"Could not initialize Anime365 community access.",
					error,
				)
			})?;
		Ok(Self { http })
	}

	pub(crate) async fn moments(
		&self,
		page: u32,
		category: Option<&MomentCategory>,
		api: &Anime365,
		adult: AdultContent,
	) -> Result<MomentPage, Error> {
		self.moments_with_ordering(
			page,
			category,
			api,
			adult,
			MomentOrdering::Recent,
		)
		.await
	}

	pub(crate) async fn trending_moments(
		&self,
		api: &Anime365,
	) -> Result<MomentPage, Error> {
		self.moments_with_ordering(
			1,
			None,
			api,
			AdultContent::Hidden,
			MomentOrdering::Popular,
		)
		.await
	}

	async fn moments_with_ordering(
		&self,
		page: u32,
		category: Option<&MomentCategory>,
		api: &Anime365,
		adult: AdultContent,
		ordering: MomentOrdering,
	) -> Result<MomentPage, Error> {
		let page = page.max(1);
		let url = moments_url(page, category, ordering)?;
		let html = self.fetch(url).await?;
		let mut result = parse_moments(&html, page)?;
		classify(&mut result.moments, api).await;
		filter_adult_moments(&mut result.moments, adult);
		Ok(result)
	}

	pub(crate) async fn moment_media(
		&self,
		moment_id: u64,
	) -> Result<MomentMedia, Error> {
		let url = Url::parse(&format!("{ORIGIN}/moments/embed/{moment_id}"))
			.map_err(|error| {
				Error::with_debug("Could not construct the Moment URL.", error)
			})?;
		parse_moment_media(&self.fetch(url).await?)
	}

	pub(crate) async fn profile(
		&self,
		user_id: u64,
		api: &Anime365,
		adult: AdultContent,
	) -> Result<ProfileEnrichment, Error> {
		let url = Url::parse(&format!("{ORIGIN}/users/{user_id}")).map_err(
			|error| {
				Error::with_debug("Could not construct the profile URL.", error)
			},
		)?;
		let mut profile = parse_profile(&self.fetch(url).await?)?;
		classify(&mut profile.moments, api).await;
		filter_adult_moments(&mut profile.moments, adult);
		Ok(profile)
	}

	async fn fetch(&self, url: Url) -> Result<String, Error> {
		let response = self
			.http
			.get(url)
			.send()
			.await
			.map_err(|error| community_error(error.without_url()))?;
		if !response.status().is_success() {
			return Err(Error::new(format!(
				"Anime365 community pages are unavailable (HTTP {}). Search and Playback remain available.",
				response.status()
			)));
		}
		response.text().await.map_err(community_error)
	}
}

fn moments_url(
	page: u32,
	category: Option<&MomentCategory>,
	ordering: MomentOrdering,
) -> Result<Url, Error> {
	let mut url = Url::parse(ORIGIN)
		.and_then(|origin| origin.join("/moments/index"))
		.map_err(|error| {
			Error::with_debug("Could not construct the Moments URL.", error)
		})?;
	let mut query = url.query_pairs_mut();
	if page > 1 {
		query.append_pair("moments-page", &page.to_string());
	}
	if let Some(category) = category {
		query.append_pair("MomentsFilter[categoryId]", &category.id);
	}
	if matches!(ordering, MomentOrdering::Popular) {
		query.append_pair("MomentsFilter[sort]", "popular");
	}
	drop(query);
	Ok(url)
}

fn filter_adult_moments(moments: &mut Vec<Moment>, adult: AdultContent) {
	if adult == AdultContent::Hidden {
		moments.retain(|moment| moment.is_adult == Some(false));
	}
}

fn parse_moments(html: &str, page: u32) -> Result<MomentPage, Error> {
	let document = Html::parse_document(html);
	let list_selector =
		selector(".m-moments-list-card[data-total], [data-moments-list]")?;
	let list = document
		.select(&list_selector)
		.next()
		.ok_or_else(changed_markup)?;
	let moment_selector = selector(".m-moment.list-item, [data-moment-id]")?;
	let moments = list
		.select(&moment_selector)
		.map(parse_moment)
		.collect::<Result<Vec<_>, _>>()?;
	let category_selector = selector(
		"#MomentsFilter_categoryId option[value], [data-moment-category]",
	)?;
	let categories = document
		.select(&category_selector)
		.filter_map(|element| {
			let id = element
				.value()
				.attr("value")
				.or_else(|| element.value().attr("data-moment-category"))?
				.to_owned();
			let label = text(element);
			(!label.is_empty() && id.parse::<u64>().is_ok())
				.then_some(MomentCategory { label, id })
		})
		.collect();
	let next_selector = selector("a[href*='moments-page='], [data-next-page]")?;
	let next_page = document
		.select(&next_selector)
		.filter_map(explicit_next_page)
		.filter(|next| *next > page)
		.min();
	Ok(MomentPage {
		moments,
		categories,
		next_page,
	})
}

fn explicit_next_page(element: ElementRef<'_>) -> Option<u32> {
	if let Some(page) = element
		.value()
		.attr("data-next-page")
		.and_then(|page| page.parse().ok())
	{
		return Some(page);
	}
	let href = element.value().attr("href")?;
	let url = Url::parse(ORIGIN).ok()?.join(href).ok()?;
	url.query_pairs().find_map(|(name, value)| {
		(name == "moments-page")
			.then(|| value.parse().ok())
			.flatten()
	})
}

pub(crate) fn validate_moments_markup(html: &str) -> Result<(), Error> {
	parse_moments(html, 1).map(drop)
}

fn parse_moment(element: ElementRef<'_>) -> Result<Moment, Error> {
	let id = element
		.value()
		.attr("data-moment-id")
		.or_else(|| element.value().id()?.strip_prefix("moment-"))
		.and_then(|id| id.parse().ok())
		.ok_or_else(changed_markup)?;
	let title =
		selected_text(element, ".m-moment__title a, [data-moment-title]")?
			.ok_or_else(changed_markup)?;
	let duration =
		selected_text(element, ".m-moment__duration, [data-duration]")?
			.unwrap_or_default();
	let thumbnail = selected_attr(
		element,
		".m-moment__thumb img, [data-thumbnail]",
		"src",
	)?
	.ok_or_else(changed_markup)?;
	let poster = selected_attr(element, ".m-moment__poster img", "src")?;
	Ok(Moment {
		id,
		title,
		duration,
		thumbnail_url: absolute(&thumbnail)?,
		series_id: poster.as_deref().and_then(series_id_from_poster),
		episode: selected_text(element, ".m-moment__episode, [data-episode]")?,
		author: selected_text(element, ".m-moment-author-name, [data-author]")?,
		age_or_date: selected_text(element, ".m-moment__date, [data-date]")?,
		views: selected_text(element, ".m-moment__views, [data-views]")?
			.as_deref()
			.and_then(number),
		is_adult: None,
	})
}

fn parse_moment_media(html: &str) -> Result<MomentMedia, Error> {
	let document = Html::parse_document(html);
	let data_selector = selector("video[data-sources]")?;
	let data_sources = document
		.select(&data_selector)
		.filter_map(|video| video.value().attr("data-sources"))
		.map(|sources| {
			serde_json::from_str::<Vec<MomentSource>>(sources).map_err(
				|error| {
					Error::with_debug(
						"Anime365 returned unreadable Moment renditions.",
						error,
					)
				},
			)
		})
		.collect::<Result<Vec<_>, _>>()?
		.into_iter()
		.flatten()
		.flat_map(|source| {
			source
				.urls
				.into_iter()
				.map(move |url| (url, Some(source.height)))
		});
	let source_selector = selector("video source[src], [data-video-url]")?;
	let source_elements = document
		.select(&source_selector)
		.filter_map(|source| {
			let url = source
				.value()
				.attr("src")
				.or_else(|| source.value().attr("data-video-url"))?;
			let height = source
				.value()
				.attr("data-height")
				.or_else(|| source.value().attr("height"))
				.and_then(|height| height.parse().ok());
			Some((url.to_owned(), height))
		})
		.collect::<Vec<_>>();
	data_sources
		.chain(source_elements)
		.filter_map(|(url, height)| {
			absolute_media(&url).ok().map(|url| (url, height))
		})
		.max_by_key(|(_, height)| height.unwrap_or_default())
		.map(|(url, height)| MomentMedia { url, height })
		.ok_or_else(changed_markup)
}

fn parse_profile(html: &str) -> Result<ProfileEnrichment, Error> {
	let document = Html::parse_document(html);
	let profile_selector = selector(".m-user-profile, [data-profile-user]")?;
	let profile = document
		.select(&profile_selector)
		.next()
		.ok_or_else(changed_markup)?;
	let avatar_url =
		selected_attr(profile, ".m-user-avatar img, [data-avatar]", "src")?
			.map(|url| absolute(&url))
			.transpose()?;
	let list_selector = selector("[data-list-status]")?;
	let lists = profile
		.select(&list_selector)
		.filter_map(parse_list_progress)
		.collect();
	let moments = parse_moments(html, 1)
		.map(|page| page.moments)
		.unwrap_or_default();
	Ok(ProfileEnrichment {
		avatar_url,
		lists,
		moments,
	})
}

fn parse_list_progress(element: ElementRef<'_>) -> Option<PublicListProgress> {
	let status = match element.value().attr("data-list-status")? {
		"watching" => PublicListStatus::Watching,
		"planned" => PublicListStatus::Planned,
		"completed" => PublicListStatus::Completed,
		"paused" => PublicListStatus::Paused,
		"dropped" => PublicListStatus::Dropped,
		_ => return None,
	};
	let title = element.value().attr("data-title").map_or_else(
		|| selected_text(element, ".title").ok().flatten(),
		|title| Some(title.to_owned()),
	)?;
	Some(PublicListProgress {
		status,
		title,
		progress: element.value().attr("data-progress").map(str::to_owned),
		score: element.value().attr("data-score").map(str::to_owned),
	})
}

fn selected_text(
	element: ElementRef<'_>,
	query: &str,
) -> Result<Option<String>, Error> {
	Ok(element
		.select(&selector(query)?)
		.next()
		.map(text)
		.filter(|text| !text.is_empty()))
}

fn selected_attr(
	element: ElementRef<'_>,
	query: &str,
	attribute: &str,
) -> Result<Option<String>, Error> {
	Ok(element
		.select(&selector(query)?)
		.next()
		.and_then(|selected| selected.value().attr(attribute))
		.map(str::to_owned))
}

fn selector(value: &str) -> Result<Selector, Error> {
	Selector::parse(value).map_err(|error| {
		Error::new(format!("Internal Anime365 selector is invalid: {error}"))
	})
}

fn text(element: ElementRef<'_>) -> String {
	element
		.text()
		.flat_map(str::split_whitespace)
		.collect::<Vec<_>>()
		.join(" ")
}

fn absolute(value: &str) -> Result<String, Error> {
	let url = Url::parse(ORIGIN)
		.and_then(|origin| origin.join(value))
		.map_err(|error| {
			Error::with_debug("Anime365 returned an invalid public URL.", error)
		})?;
	if url.scheme() != "https"
		|| !url.host_str().is_some_and(is_public_page_host)
	{
		return Err(Error::new("Anime365 returned an unsafe public URL."));
	}
	Ok(url.into())
}

fn absolute_media(value: &str) -> Result<String, Error> {
	let url = Url::parse(ORIGIN)
		.and_then(|origin| origin.join(value))
		.map_err(|error| {
			Error::with_debug("Anime365 returned an invalid Moment URL.", error)
		})?;
	if url.scheme() != "https"
		|| !url.host_str().is_some_and(|host| {
			is_public_page_host(host)
				|| host == "quantum-phantom-moon.ru"
				|| host.ends_with(".quantum-phantom-moon.ru")
		}) {
		return Err(Error::new("Anime365 returned an unsafe Moment URL."));
	}
	Ok(url.into())
}

fn is_public_page_host(host: &str) -> bool {
	matches!(host, "anime365.ru" | "smotret-anime.org")
}

fn series_id_from_poster(path: &str) -> Option<u64> {
	path.split("/posters/")
		.nth(1)?
		.split('.')
		.next()?
		.parse()
		.ok()
}

fn number(value: &str) -> Option<u64> {
	let digits = value
		.chars()
		.filter(char::is_ascii_digit)
		.collect::<String>();
	(!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn community_error(error: impl std::error::Error) -> Error {
	Error::with_debug(
		"Anime365 community pages are unavailable. Search and Playback remain available.",
		error,
	)
}

fn changed_markup() -> Error {
	Error::new(
		"Anime365 community markup changed. Moments and profile enrichment are temporarily unavailable; Search and Playback remain available.",
	)
}

#[cfg(test)]
#[path = "community_tests.rs"]
mod tests;
