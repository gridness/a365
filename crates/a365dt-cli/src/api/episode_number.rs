use std::fmt;

use serde::{
	Deserializer,
	de::{self, Visitor},
};

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
where
	D: Deserializer<'de>,
{
	deserializer.deserialize_any(EpisodeNumberVisitor)
}

struct EpisodeNumberVisitor;

impl Visitor<'_> for EpisodeNumberVisitor {
	type Value = String;

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("an Episode number encoded as a string or number")
	}

	fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		Ok(value.to_owned())
	}

	fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		Ok(value)
	}

	fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		Ok(value.to_string())
	}

	fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		Ok(value.to_string())
	}

	fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
	where
		E: de::Error,
	{
		if !value.is_finite() {
			return Err(E::custom("Episode number must be finite"));
		}
		Ok(value.to_string())
	}
}
