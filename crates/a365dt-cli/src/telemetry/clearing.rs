use std::io::{self, IsTerminal};

use chrono::{
	DateTime, Datelike, Days, LocalResult, NaiveDate, NaiveDateTime, TimeDelta,
	TimeZone,
};

use crate::error::Error;

const INVALID_INTERVAL: &str = "Invalid telemetry clear interval. Use a \
	positive duration such as `30m` or `30 minutes`, or `today`, `this week`, \
	`this month`, or `this year`.";

#[derive(Debug, Eq, PartialEq)]
pub(super) enum PreparedClear {
	All {
		cleared_at_ms: u64,
	},
	Since {
		cleared_at_ms: u64,
		cutoff_ms: u64,
		expression: String,
	},
}

pub(crate) enum ClearRequest {
	All(FullClearPermission),
	Since(Vec<String>),
}

#[derive(Clone, Copy)]
pub(crate) enum FullClearPermission {
	Ask,
	Preauthorized,
}

#[derive(Clone, Copy)]
pub(super) enum TerminalAccess {
	Interactive,
	NonInteractive,
}

impl TerminalAccess {
	pub(super) fn detect() -> Self {
		if io::stdin().is_terminal() && io::stdout().is_terminal() {
			Self::Interactive
		} else {
			Self::NonInteractive
		}
	}
}

pub(super) fn prepare_all<Tz: TimeZone>(
	now: DateTime<Tz>,
) -> Result<PreparedClear, Error> {
	Ok(PreparedClear::All {
		cleared_at_ms: u64::try_from(now.timestamp_millis())
			.map_err(|_| invalid_interval())?,
	})
}

pub(super) fn authorize_full_clear(
	permission: FullClearPermission,
	terminals: TerminalAccess,
	confirm: impl FnOnce() -> Result<bool, Error>,
) -> Result<bool, Error> {
	match (permission, terminals) {
		(FullClearPermission::Preauthorized, _) => Ok(true),
		(FullClearPermission::Ask, TerminalAccess::Interactive) => confirm(),
		(FullClearPermission::Ask, TerminalAccess::NonInteractive) => {
			Err(Error::new(
				"Run `a365 telemetry clear --yes` to clear all local \
				 telemetry without terminal confirmation.",
			))
		}
	}
}

pub(super) fn prepare_since<S, Tz>(
	values: &[S],
	now: DateTime<Tz>,
) -> Result<PreparedClear, Error>
where
	S: AsRef<str>,
	Tz: TimeZone,
{
	let cleared_at_ms = u64::try_from(now.timestamp_millis())
		.map_err(|_| invalid_interval())?;
	let expression = values
		.iter()
		.flat_map(|value| value.as_ref().split_ascii_whitespace())
		.collect::<Vec<_>>()
		.join(" ")
		.to_ascii_lowercase();
	let boundary = match expression.as_str() {
		"today" => Some(now.date_naive()),
		"this week" => Some(
			now.date_naive()
				.checked_sub_days(Days::new(u64::from(
					now.weekday().num_days_from_monday(),
				)))
				.ok_or_else(invalid_interval)?,
		),
		"this month" => Some(
			NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
				.ok_or_else(invalid_interval)?,
		),
		"this year" => Some(
			NaiveDate::from_ymd_opt(now.year(), 1, 1)
				.ok_or_else(invalid_interval)?,
		),
		_ => None,
	};
	let cutoff_ms = match boundary {
		Some(date) => calendar_cutoff(now.timezone(), date)?,
		None => elapsed_cutoff(&expression, cleared_at_ms)?,
	};
	Ok(PreparedClear::Since {
		cleared_at_ms,
		cutoff_ms,
		expression,
	})
}

fn elapsed_cutoff(expression: &str, now_ms: u64) -> Result<u64, Error> {
	let words = expression.split_ascii_whitespace().collect::<Vec<_>>();
	let (amount, unit_ms) = match words.as_slice() {
		[compact] => {
			let amount_length =
				compact.len().checked_sub(1).ok_or_else(invalid_interval)?;
			let (amount, unit) = compact
				.split_at_checked(amount_length)
				.ok_or_else(invalid_interval)?;
			let unit_ms = match unit {
				"m" => 60_000,
				"h" => 3_600_000,
				"d" => 86_400_000,
				"w" => 604_800_000,
				_ => return Err(invalid_interval()),
			};
			(amount, unit_ms)
		}
		[amount, unit] => {
			let unit_ms = match *unit {
				"minute" | "minutes" => 60_000,
				"hour" | "hours" => 3_600_000,
				"day" | "days" => 86_400_000,
				"week" | "weeks" => 604_800_000,
				_ => return Err(invalid_interval()),
			};
			(*amount, unit_ms)
		}
		_ => return Err(invalid_interval()),
	};
	let amount = amount
		.parse::<u64>()
		.ok()
		.filter(|_| amount.bytes().all(|byte| byte.is_ascii_digit()))
		.filter(|amount| *amount > 0)
		.ok_or_else(invalid_interval)?;
	let elapsed_ms =
		amount.checked_mul(unit_ms).ok_or_else(invalid_interval)?;
	now_ms.checked_sub(elapsed_ms).ok_or_else(invalid_interval)
}

fn calendar_cutoff<Tz: TimeZone>(
	timezone: Tz,
	date: NaiveDate,
) -> Result<u64, Error> {
	let instant =
		resolve_boundary(date, |local| timezone.from_local_datetime(&local))
			.ok_or_else(|| {
				Error::new(
					"Could not resolve the local telemetry clear boundary.",
				)
			})?;
	u64::try_from(instant.timestamp_millis()).map_err(|_| invalid_interval())
}

pub(super) fn resolve_boundary<T>(
	date: NaiveDate,
	mut resolve: impl FnMut(NaiveDateTime) -> LocalResult<T>,
) -> Option<T>
where
	T: Ord,
{
	let midnight = date.and_hms_opt(0, 0, 0)?;
	for second in 0..86_400 {
		let local = midnight.checked_add_signed(TimeDelta::seconds(second))?;
		match resolve(local) {
			LocalResult::Single(instant) => return Some(instant),
			LocalResult::Ambiguous(first, second) => {
				return Some(first.min(second));
			}
			LocalResult::None => {}
		}
	}
	None
}

fn invalid_interval() -> Error {
	Error::new(INVALID_INTERVAL)
}
