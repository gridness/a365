use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(
	Clone,
	Copy,
	Debug,
	Default,
	Deserialize,
	Eq,
	Hash,
	Ord,
	PartialEq,
	PartialOrd,
	Serialize,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContentSource {
	#[default]
	Anime365,
	H365,
}

#[derive(
	Clone,
	Copy,
	Debug,
	Deserialize,
	Eq,
	Hash,
	Ord,
	PartialEq,
	PartialOrd,
	Serialize,
)]
pub(crate) struct SeriesKey {
	pub(crate) source: ContentSource,
	pub(crate) id: u64,
}

impl ContentSource {
	pub(crate) const fn as_str(self) -> &'static str {
		match self {
			Self::Anime365 => "anime365",
			Self::H365 => "h365",
		}
	}

	pub(crate) fn from_storage(value: &str) -> Option<Self> {
		match value {
			"anime365" => Some(Self::Anime365),
			"h365" => Some(Self::H365),
			_ => None,
		}
	}
}

impl fmt::Display for ContentSource {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Anime365 => "Anime365",
			Self::H365 => "H365",
		})
	}
}

impl SeriesKey {
	pub(crate) const fn new(source: ContentSource, id: u64) -> Self {
		Self { source, id }
	}
}
