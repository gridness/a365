use std::cmp::Reverse;

use chrono::{
	DateTime, Datelike, Days, Duration, Local, LocalResult, NaiveDate,
	NaiveDateTime, NaiveTime, TimeZone,
};
use serde::Deserialize;

use super::{Client, ListStatus, MediaTitle};
use crate::{error::Error, preferences::AdultContent};

const PAGE_SIZE: u8 = 50;
const MAX_PAGES: u8 = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScheduleEntry {
	pub airing_at: i64,
	pub episode: u32,
	pub media_id: u64,
	pub mal_id: Option<u64>,
	pub title: MediaTitle,
	pub is_adult: Option<bool>,
	pub list_status: Option<ListStatus>,
	pub priority: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ScheduleData {
	page: SchedulePage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulePage {
	page_info: PageInfo,
	airing_schedules: Vec<AiringSchedule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
	has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiringSchedule {
	airing_at: i64,
	episode: u32,
	media: ScheduleMedia,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleMedia {
	id: u64,
	id_mal: Option<u64>,
	is_adult: Option<bool>,
	title: MediaTitle,
	media_list_entry: Option<ScheduleListEntry>,
}

#[derive(Deserialize)]
struct ScheduleListEntry {
	status: Option<ListStatus>,
	#[serde(default)]
	priority: u8,
}

pub(crate) async fn current_week(
	adult: AdultContent,
) -> Result<Vec<ScheduleEntry>, Error> {
	let client = Client::connected()?.unwrap_or(Client::public()?);
	current_week_at(&client, adult, Local::now()).await
}

async fn current_week_at<Tz>(
	client: &Client,
	adult: AdultContent,
	now: DateTime<Tz>,
) -> Result<Vec<ScheduleEntry>, Error>
where
	Tz: TimeZone,
	Tz::Offset: Send + Sync,
{
	let (start, end) = week_bounds(&now)?;
	let (start, end) = query_bounds(start.timestamp(), end.timestamp());
	let mut entries = fetch(client, start, end).await?;
	arrange(&mut entries, adult, now.timezone());
	Ok(entries)
}

async fn fetch(
	client: &Client,
	start: i64,
	end: i64,
) -> Result<Vec<ScheduleEntry>, Error> {
	const QUERY: &str = r#"query WeeklySchedule($page: Int!, $perPage: Int!, $start: Int!, $end: Int!) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    airingSchedules(airingAt_greater: $start, airingAt_lesser: $end, sort: TIME) {
      airingAt episode
      media { id idMal isAdult title { userPreferred romaji english native }
        mediaListEntry { status priority } }
    }
  }
}"#;
	let mut result = Vec::new();
	for page in 1..=MAX_PAGES {
		let response = client
			.query::<ScheduleData, _>(
				QUERY,
				serde_json::json!({
					"page": page,
					"perPage": PAGE_SIZE,
					"start": start,
					"end": end,
				}),
			)
			.await?
			.page;
		result.extend(schedule_entries(response.airing_schedules));
		if !response.page_info.has_next_page {
			return Ok(result);
		}
	}
	Err(Error::new(
		"AniList returned more weekly schedule pages than a365 can safely load.",
	))
}

const fn query_bounds(start: i64, end: i64) -> (i64, i64) {
	(start.saturating_sub(1), end)
}

fn schedule_entries(schedules: Vec<AiringSchedule>) -> Vec<ScheduleEntry> {
	schedules
		.into_iter()
		.map(|schedule| {
			let list = schedule.media.media_list_entry;
			ScheduleEntry {
				airing_at: schedule.airing_at,
				episode: schedule.episode,
				media_id: schedule.media.id,
				mal_id: schedule.media.id_mal,
				title: schedule.media.title,
				is_adult: schedule.media.is_adult,
				list_status: list.as_ref().and_then(|entry| entry.status),
				priority: list.map_or(0, |entry| entry.priority),
			}
		})
		.collect()
}

fn week_bounds<Tz>(
	now: &DateTime<Tz>,
) -> Result<(DateTime<Tz>, DateTime<Tz>), Error>
where
	Tz: TimeZone,
{
	let days_from_monday = u64::from(now.weekday().num_days_from_monday());
	let monday = now
		.date_naive()
		.checked_sub_days(Days::new(days_from_monday))
		.ok_or_else(|| Error::new("Could not calculate the current week."))?;
	let next_monday = monday
		.checked_add_days(Days::new(7))
		.ok_or_else(|| Error::new("Could not calculate the current week."))?;
	Ok((
		start_of_day(now.timezone(), monday)?,
		start_of_day(now.timezone(), next_monday)?,
	))
}

fn start_of_day<Tz>(
	timezone: Tz,
	date: NaiveDate,
) -> Result<DateTime<Tz>, Error>
where
	Tz: TimeZone,
{
	let midnight = NaiveDateTime::new(date, NaiveTime::MIN);
	for minutes in 0..=180 {
		let candidate = midnight + Duration::minutes(minutes);
		match timezone.from_local_datetime(&candidate) {
			LocalResult::Single(value) => return Ok(value),
			LocalResult::Ambiguous(first, second) => {
				return Ok(first.min(second));
			}
			LocalResult::None => {}
		}
	}
	Err(Error::new(
		"Could not resolve the local timezone at the week boundary.",
	))
}

fn arrange<Tz>(
	entries: &mut Vec<ScheduleEntry>,
	adult: AdultContent,
	timezone: Tz,
) where
	Tz: TimeZone,
{
	if adult == AdultContent::Hidden {
		entries.retain(|entry| entry.is_adult == Some(false));
	}
	entries.sort_by(|left, right| {
		let left_date = local_date(left.airing_at, &timezone);
		let right_date = local_date(right.airing_at, &timezone);
		left_date
			.cmp(&right_date)
			.then_with(|| personal_rank(left).cmp(&personal_rank(right)))
			.then_with(|| left.airing_at.cmp(&right.airing_at))
			.then_with(|| left.title.display().cmp(right.title.display()))
			.then_with(|| left.media_id.cmp(&right.media_id))
	})
}

fn local_date<Tz>(timestamp: i64, timezone: &Tz) -> Option<NaiveDate>
where
	Tz: TimeZone,
{
	timezone
		.timestamp_opt(timestamp, 0)
		.single()
		.map(|value| value.date_naive())
}

fn personal_rank(entry: &ScheduleEntry) -> (u8, Reverse<u8>) {
	match entry.list_status {
		Some(ListStatus::Current | ListStatus::Repeating) => (0, Reverse(0)),
		Some(ListStatus::Planning) => (1, Reverse(entry.priority)),
		None => (2, Reverse(0)),
		Some(ListStatus::Paused) => (3, Reverse(0)),
		Some(ListStatus::Completed) => (4, Reverse(0)),
		Some(ListStatus::Dropped) => (5, Reverse(0)),
	}
}

#[cfg(test)]
#[path = "timetable_tests.rs"]
mod tests;
