use pretty_assertions::assert_eq;

use super::{
	ApiError, Embed, Envelope, Episode, MediaOption, Series, Translation,
	normalize_url, series_key_from_url, source_translations,
};
use crate::content::{ContentSource, SeriesKey};

#[test]
fn resolves_relative_assets_on_media_origin() {
	assert_eq!(
		normalize_url(
			ContentSource::Anime365,
			"/episodeTranslations/3954619.ass?willcache",
		)
		.unwrap(),
		"https://smotret-anime.org/episodeTranslations/3954619.ass?willcache"
			.parse()
			.unwrap()
	);
}

#[test]
fn routes_posters_around_the_challenged_web_origin() {
	assert_eq!(
		normalize_url(
			ContentSource::Anime365,
			"https://anime365.ru/posters/30887.small.jpg",
		)
		.unwrap(),
		"https://smotret-anime.org/posters/30887.small.jpg"
			.parse()
			.unwrap()
	);
}

#[test]
fn parses_only_official_series_urls() {
	assert_eq!(
		series_key_from_url(
			"https://smotret-anime.org/catalog/road-of-naruto-30887/"
		),
		Some(SeriesKey::new(ContentSource::Anime365, 30887))
	);
	assert_eq!(
		series_key_from_url("https://example.com/catalog/title-1"),
		None
	);
	assert_eq!(
		series_key_from_url("https://anime365.ru/catalog/title-1/2-seriya-3"),
		None
	);
	assert_eq!(
		series_key_from_url("https://hentai365.ru/catalog/title-42"),
		Some(SeriesKey::new(ContentSource::H365, 42))
	);
}

#[test]
fn parses_official_series_fixture() {
	let actual: Envelope<Vec<Series>> = serde_json::from_str(
        r#"{"data":[{"id":30887,"title":"ROAD OF NARUTO","year":2022,"typeTitle":"ONA","numberOfEpisodes":1,"myAnimeListId":53236,"anilistId":166946,"posterUrlSmall":"https://anime365.ru/posters/30887.small.jpg","episodes":[{"id":292232,"episodeInt":"1","episodeFull":"ONA 1"}]}]}"#,
    )
    .unwrap();

	assert_eq!(
		actual.data.unwrap(),
		vec![Series {
			source: ContentSource::Anime365,
			id: 30887,
			title: "ROAD OF NARUTO".into(),
			year: Some(2022),
			type_title: Some("ONA".into()),
			number_of_episodes: Some(1),
			my_anime_list_id: Some(53_236),
			anilist_id: Some(166_946),
			poster_url_small: Some(
				"https://anime365.ru/posters/30887.small.jpg".into(),
			),
			episodes: vec![Episode {
				source: ContentSource::Anime365,
				id: 292232,
				episode_int: "1".into(),
				episode_full: "ONA 1".into(),
			}],
		}]
	);
}

#[test]
fn parses_source_qualified_h365_catalogue_and_series_fixtures() {
	let fixture = r#"{"data":[{"id":7,"title":"H365 title","year":2026,"typeTitle":"ONA","numberOfEpisodes":2,"myAnimeListId":null,"anilistId":null,"episodes":[{"id":70,"episodeInt":"1","episodeFull":"Episode 1"}]}]}"#;
	let client =
		super::Anime365::h365("fixture-token".into(), Default::default())
			.unwrap();
	let parsed = serde_json::from_str::<Envelope<Vec<Series>>>(fixture)
		.unwrap()
		.data
		.unwrap();
	let expected = vec![Series {
		source: ContentSource::H365,
		id: 7,
		title: "H365 title".into(),
		year: Some(2026),
		type_title: Some("ONA".into()),
		number_of_episodes: Some(2),
		my_anime_list_id: None,
		anilist_id: None,
		poster_url_small: None,
		episodes: vec![Episode {
			source: ContentSource::H365,
			id: 70,
			episode_int: "1".into(),
			episode_full: "Episode 1".into(),
		}],
	}];

	assert_eq!(client.source_series(parsed), expected);
}

#[test]
fn parses_source_qualified_h365_translation_and_embed_fixtures() {
	let translations = serde_json::from_str::<Envelope<Vec<Translation>>>(
		r#"{"data":[{"id":8,"episodeId":70,"typeKind":"sub","typeLang":"ru","authorsSummary":"Team"}]}"#,
	)
	.unwrap()
	.data
	.unwrap();
	let embed = serde_json::from_str::<Envelope<Embed>>(
		r#"{"data":{"download":[{"height":720,"url":"https://hentai365.ru/video/8.mp4"}],"subtitlesUrl":"/episodeTranslations/8.ass"}}"#,
	)
	.unwrap()
	.data
	.unwrap();

	assert_eq!(
		(
			source_translations(ContentSource::H365, translations),
			embed
		),
		(
			vec![Translation {
				source: ContentSource::H365,
				id: 8,
				episode_id: 70,
				kind: "sub".into(),
				language: "ru".into(),
				authors_summary: "Team".into(),
			}],
			Embed {
				download: vec![MediaOption {
					height: 720,
					url: Some("https://hentai365.ru/video/8.mp4".into()),
				}],
				subtitles_url: Some("/episodeTranslations/8.ass".into()),
			},
		),
	);
}

#[test]
fn parses_h365_failure_fixture_without_losing_the_source_context() {
	let failure = serde_json::from_str::<Envelope<Vec<Series>>>(
		r#"{"data":null,"error":{"code":403,"message":"Access denied"}}"#,
	)
	.unwrap();

	assert_eq!(
		failure,
		Envelope {
			data: None,
			error: Some(ApiError {
				code: 403,
				message: "Access denied".into(),
			}),
		},
	);
	assert_eq!(
		super::api_origin(ContentSource::H365),
		"https://hentai365.ru/api"
	);
	assert_eq!(
		normalize_url(ContentSource::H365, "/episodeTranslations/8.ass")
			.unwrap(),
		"https://hentai365.ru/episodeTranslations/8.ass"
			.parse()
			.unwrap(),
	);
}
