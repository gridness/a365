use pretty_assertions::assert_eq;

use super::{
	Moment, MomentCategory, MomentMedia, ProfileEnrichment, PublicListProgress,
	PublicListStatus, filter_adult_moments, parse_moment_media, parse_moments,
	parse_profile,
};
use crate::preferences::AdultContent;

const FEED: &str = r#"
<select id="MomentsFilter_categoryId"><option value="">All</option><option value="1">Recent</option></select>
<a class="load-more" href="/moments/index?moments-page=2" data-page="1">Next</a>
<div class="m-moments-list-card" data-total="3">
  <div class="m-moment list-item" id="moment-250103">
    <a class="m-moment__img" href="/moments/250103">
      <div class="m-moment__thumb"><img src="/moments/thumbnail/250103.jpg"></div>
      <div class="m-moment__poster"><img src="/posters/40024.123.jpg"></div>
      <div class="m-moment__duration">1:09</div>
    </a>
    <div class="m-moment__title"><a>Ghosts</a></div>
    <div class="m-moment__episode">from episode 7 · Smoking Behind the Supermarket</div>
    <span class="m-moment-author-name">Kotrieren</span>
    <span class="m-moment__date">7 hours ago</span>
    <span class="m-moment__views">1 203 views</span>
  </div>
  <div class="m-moment list-item" data-moment-id="250104">
    <div class="m-moment__thumb"><img src="/moments/thumbnail/250104.jpg"></div>
    <div class="m-moment__title"><a>Untethered clip</a></div>
  </div>
</div>
"#;

#[test]
fn parses_sanitized_moment_feed_pagination_categories_and_missing_fields() {
	let page = parse_moments(FEED, 1).unwrap();

	assert_eq!(
		page,
		super::MomentPage {
			moments: vec![
				Moment {
					id: 250_103,
					title: "Ghosts".into(),
					duration: "1:09".into(),
					thumbnail_url:
						"https://smotret-anime.org/moments/thumbnail/250103.jpg"
							.into(),
					series_id: Some(40_024),
					episode: Some(
						"from episode 7 · Smoking Behind the Supermarket"
							.into()
					),
					author: Some("Kotrieren".into()),
					age_or_date: Some("7 hours ago".into()),
					views: Some(1_203),
					is_adult: None,
				},
				Moment {
					id: 250_104,
					title: "Untethered clip".into(),
					duration: String::new(),
					thumbnail_url:
						"https://smotret-anime.org/moments/thumbnail/250104.jpg"
							.into(),
					series_id: None,
					episode: None,
					author: None,
					age_or_date: None,
					views: None,
					is_adult: None,
				},
			],
			categories: vec![MomentCategory {
				label: "Recent".into(),
				id: "1".into(),
			}],
			next_page: Some(2),
		},
	);
}

#[test]
fn pagination_stops_on_a_short_final_page_without_an_explicit_next_link() {
	let final_page = FEED.replace(
		"<a class=\"load-more\" href=\"/moments/index?moments-page=2\" data-page=\"1\">Next</a>",
		"",
	);

	assert_eq!(parse_moments(&final_page, 1).unwrap().next_page, None);
}

#[test]
fn chooses_the_highest_official_moment_rendition() {
	let media = parse_moment_media(
		r#"<video data-sources="[{&quot;height&quot;:480,&quot;urls&quot;:[&quot;https://sand.quantum-phantom-moon.ru/route/480&quot;]},{&quot;height&quot;:1080,&quot;urls&quot;:[&quot;https://sand.quantum-phantom-moon.ru/route/1080?m=1&amp;r=redacted&quot;]}]"></video>"#,
	)
	.unwrap();

	assert_eq!(
		media,
		MomentMedia {
			url:
				"https://sand.quantum-phantom-moon.ru/route/1080?m=1&r=redacted"
					.into(),
			height: Some(1080),
		},
	);
}

#[test]
fn rejects_untrusted_moment_media_hosts() {
	let error = parse_moment_media(
		r#"<video data-sources='[{"height":1080,"urls":["https://untrusted.example/moment"]}]'></video>"#,
	)
	.unwrap_err();

	assert!(error.message().contains("community markup changed"));
}

#[test]
fn adult_filtering_hides_unclassified_moments() {
	let mut moments = parse_moments(FEED, 1).unwrap().moments;
	moments[0].is_adult = Some(false);
	moments[1].is_adult = Some(true);
	moments.push(Moment {
		id: 3,
		title: "Unknown".into(),
		duration: String::new(),
		thumbnail_url: "https://smotret-anime.org/moments/3.jpg".into(),
		series_id: None,
		episode: None,
		author: None,
		age_or_date: None,
		views: None,
		is_adult: None,
	});

	filter_adult_moments(&mut moments, AdultContent::Hidden);

	assert_eq!(
		moments
			.into_iter()
			.map(|moment| moment.id)
			.collect::<Vec<_>>(),
		vec![250_103]
	);
}

#[test]
fn parses_profile_lists_as_declared_progress_not_history() {
	let profile = parse_profile(
		r#"<section class="m-user-profile"><div class="m-user-avatar"><img src="/users/avatars/9.jpg"></div><div data-list-status="watching" data-title="Frieren" data-progress="3/28" data-score="9"></div><div data-list-status="planned" data-title="Orb"></div></section>"#,
	)
	.unwrap();

	assert_eq!(
		profile,
		ProfileEnrichment {
			avatar_url: Some(
				"https://smotret-anime.org/users/avatars/9.jpg".into()
			),
			lists: vec![
				PublicListProgress {
					status: PublicListStatus::Watching,
					title: "Frieren".into(),
					progress: Some("3/28".into()),
					score: Some("9".into()),
				},
				PublicListProgress {
					status: PublicListStatus::Planned,
					title: "Orb".into(),
					progress: None,
					score: None,
				},
			],
			moments: Vec::new(),
		},
	);
}

#[test]
fn profile_enrichment_recognizes_all_five_public_list_statuses() {
	let profile = parse_profile(
		r#"<section data-profile-user><div data-list-status="watching" data-title="Watching"></div><div data-list-status="planned" data-title="Planned"></div><div data-list-status="completed" data-title="Completed"></div><div data-list-status="paused" data-title="Paused"></div><div data-list-status="dropped" data-title="Dropped"></div></section>"#,
	)
	.unwrap();

	assert_eq!(
		profile.lists,
		vec![
			PublicListProgress {
				status: PublicListStatus::Watching,
				title: "Watching".into(),
				progress: None,
				score: None,
			},
			PublicListProgress {
				status: PublicListStatus::Planned,
				title: "Planned".into(),
				progress: None,
				score: None,
			},
			PublicListProgress {
				status: PublicListStatus::Completed,
				title: "Completed".into(),
				progress: None,
				score: None,
			},
			PublicListProgress {
				status: PublicListStatus::Paused,
				title: "Paused".into(),
				progress: None,
				score: None,
			},
			PublicListProgress {
				status: PublicListStatus::Dropped,
				title: "Dropped".into(),
				progress: None,
				score: None,
			},
		],
	);
}

#[test]
fn changed_markup_is_a_distinct_isolated_error() {
	let error =
		parse_moments("<main>ordinary Anime365 remains usable</main>", 1)
			.unwrap_err();

	assert!(
		error.message().contains("community markup changed")
			&& error
				.message()
				.contains("Search and Playback remain available")
	);
}
