use super::explicit_adult_content;
use crate::community::Moment;

fn moment(title: &str, episode: Option<&str>) -> Moment {
	Moment {
		id: 1,
		title: title.into(),
		duration: "0:30".into(),
		thumbnail_url: "https://smotret-anime.org/moments/1.jpg".into(),
		series_id: Some(1),
		episode: episode.map(str::to_owned),
		author: None,
		age_or_date: None,
		views: None,
		is_adult: None,
	}
}

#[test]
fn explicit_hentai_markers_cover_titles_and_episode_context() {
	assert!(explicit_adult_content(&moment("Hentai", None)));
	assert!(explicit_adult_content(&moment("ХЕНТАЙ", None)));
	assert!(explicit_adult_content(&moment("Opening", Some("Hentai"))));
	assert!(!explicit_adult_content(&moment("Moment 18+", None)));
	assert!(!explicit_adult_content(&moment("Ecchi erotica", None)));
	assert!(!explicit_adult_content(&moment(
		"Ordinary scene",
		Some("Episode 1"),
	)));
}
