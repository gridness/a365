use chrono::{TimeZone, Utc};
use pretty_assertions::assert_eq;

use super::{
	ScheduleData, ScheduleEntry, arrange, query_bounds, schedule_entries,
	week_bounds,
};
use crate::{
	anilist::{ListStatus, MediaTitle},
	preferences::AdultContent,
};

fn entry(
	id: u64,
	at: i64,
	status: Option<ListStatus>,
	priority: u8,
	is_adult: Option<bool>,
) -> ScheduleEntry {
	ScheduleEntry {
		airing_at: at,
		episode: 1,
		media_id: id,
		mal_id: None,
		title: MediaTitle {
			user_preferred: Some(format!("Series {id}")),
			romaji: None,
			english: None,
			native: None,
		},
		is_adult,
		list_status: status,
		priority,
	}
}

#[test]
fn current_week_uses_monday_through_next_monday_in_the_given_timezone() {
	let now = Utc.with_ymd_and_hms(2026, 8, 26, 12, 0, 0).unwrap();
	let (start, end) = week_bounds(&now).unwrap();

	assert_eq!(
		(start, end),
		(
			Utc.with_ymd_and_hms(2026, 8, 24, 0, 0, 0).unwrap(),
			Utc.with_ymd_and_hms(2026, 8, 31, 0, 0, 0).unwrap(),
		)
	);
}

#[test]
fn graphql_query_includes_the_exact_week_start_and_excludes_the_end() {
	assert_eq!(query_bounds(1_000, 2_000), (999, 2_000));
	assert_eq!(query_bounds(i64::MIN, i64::MAX), (i64::MIN, i64::MAX));
}

#[test]
fn week_boundaries_follow_daylight_saving_changes() {
	let timezone = chrono_tz::America::New_York;
	let now = timezone.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
	let (start, end) = week_bounds(&now).unwrap();

	assert_eq!(
		(
			start.to_rfc3339(),
			end.to_rfc3339(),
			end.timestamp() - start.timestamp(),
		),
		(
			"2026-03-02T00:00:00-05:00".into(),
			"2026-03-09T00:00:00-04:00".into(),
			7 * 86_400 - 3_600,
		),
	);
}

#[test]
fn personalization_orders_each_day_and_adult_filtering_fails_closed() {
	let day = 1_788_000_000;
	let mut entries = vec![
		entry(1, day + 1, None, 0, Some(false)),
		entry(2, day + 2, Some(ListStatus::Planning), 2, Some(false)),
		entry(3, day + 3, Some(ListStatus::Current), 0, Some(false)),
		entry(4, day + 4, Some(ListStatus::Planning), 9, Some(false)),
		entry(5, day + 5, Some(ListStatus::Paused), 0, Some(false)),
		entry(6, day + 6, Some(ListStatus::Current), 0, None),
	];

	arrange(&mut entries, AdultContent::Hidden, Utc);

	assert_eq!(
		entries
			.into_iter()
			.map(|entry| entry.media_id)
			.collect::<Vec<_>>(),
		vec![3, 4, 2, 1, 5],
	);
}

#[test]
fn graphql_fixtures_cover_public_and_authenticated_schedule_context() {
	let parse = |list_entry: &str| {
		let fixture = format!(
			r#"{{"data":{{"Page":{{"pageInfo":{{"hasNextPage":false}},"airingSchedules":[{{"airingAt":1788000000,"episode":4,"media":{{"id":154587,"idMal":52991,"isAdult":false,"title":{{"userPreferred":"Frieren","romaji":null,"english":null,"native":null}},"mediaListEntry":{list_entry}}}}}]}}}},"errors":[]}}"#,
		);
		let envelope = serde_json::from_str::<
			super::super::GraphQlEnvelope<ScheduleData>,
		>(&fixture)
		.unwrap();
		schedule_entries(envelope.data.unwrap().page.airing_schedules)
	};
	let title = MediaTitle {
		user_preferred: Some("Frieren".into()),
		romaji: None,
		english: None,
		native: None,
	};

	assert_eq!(
		(parse("null"), parse(r#"{"status":"CURRENT","priority":9}"#)),
		(
			vec![ScheduleEntry {
				airing_at: 1_788_000_000,
				episode: 4,
				media_id: 154_587,
				mal_id: Some(52_991),
				title: title.clone(),
				is_adult: Some(false),
				list_status: None,
				priority: 0,
			}],
			vec![ScheduleEntry {
				airing_at: 1_788_000_000,
				episode: 4,
				media_id: 154_587,
				mal_id: Some(52_991),
				title,
				is_adult: Some(false),
				list_status: Some(ListStatus::Current),
				priority: 9,
			}],
		),
	);
}

#[test]
fn personalization_keeps_every_status_group_in_the_confirmed_order() {
	let day = 1_788_000_000;
	let mut entries = vec![
		entry(1, day + 1, Some(ListStatus::Dropped), 0, Some(false)),
		entry(2, day + 2, Some(ListStatus::Completed), 0, Some(false)),
		entry(3, day + 3, Some(ListStatus::Paused), 0, Some(false)),
		entry(4, day + 4, None, 0, Some(false)),
		entry(5, day + 5, Some(ListStatus::Planning), 1, Some(false)),
		entry(6, day + 6, Some(ListStatus::Repeating), 0, Some(false)),
	];

	arrange(&mut entries, AdultContent::Visible, Utc);

	assert_eq!(
		entries
			.into_iter()
			.map(|entry| entry.media_id)
			.collect::<Vec<_>>(),
		vec![6, 5, 4, 3, 2, 1],
	);
}
