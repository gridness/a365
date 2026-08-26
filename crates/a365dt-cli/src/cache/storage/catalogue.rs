use std::collections::{BTreeMap, HashMap};

use super::{Store, read_error, u64_from};
use crate::{
	api::Series,
	cache::{catalogue::Catalogue, writer::LoadedCatalogue},
	content::{ContentSource, SeriesKey},
	error::Error,
};

struct StoredSeries {
	source: String,
	id: i64,
	title: String,
	year: Option<i64>,
	type_title: Option<String>,
	episode_count: Option<i64>,
	my_anime_list_id: Option<i64>,
	anilist_id: Option<i64>,
}

impl Store {
	pub(crate) async fn load_catalogue(
		&self,
	) -> Result<LoadedCatalogue, Error> {
		let Ok(available) = &self.available else {
			return Ok(LoadedCatalogue::unavailable());
		};
		let mut transaction =
			available.pool.begin().await.map_err(read_error)?;
		let revision: i64 = sqlx::query_scalar(
			"SELECT revision FROM catalogue_state WHERE singleton = 1",
		)
		.fetch_one(&mut *transaction)
		.await
		.map_err(read_error)?;
		let source_states = sqlx::query_as::<_, (String, i64, Option<i64>)>(
			"SELECT source, current_generation, refreshed_at \
			 FROM catalogue_source_state ORDER BY source",
		)
		.fetch_all(&mut *transaction)
		.await
		.map_err(read_error)?;
		let rows = sqlx::query_as::<_, StoredRow>(
			"SELECT series.source, series.id, series.title, series.year, \
			 series.type_title, series.episode_count, \
			 series.my_anime_list_id, series.anilist_id, \
			 revision \
			 FROM series JOIN catalogue_source_state AS source_state \
			 ON source_state.source = series.source \
			 ORDER BY CASE WHEN refresh_generation = \
			 source_state.current_generation THEN 0 ELSE 1 END, \
			 refresh_position, discovery_order, series.source, series.id",
		)
		.fetch_all(&mut *transaction)
		.await
		.map_err(read_error)?;
		let aliases = sqlx::query_as::<_, (String, String, i64)>(
			"SELECT query, series_source, series_id \
			 FROM aliases ORDER BY query",
		)
		.fetch_all(&mut *transaction)
		.await
		.map_err(read_error)?;
		transaction.commit().await.map_err(read_error)?;

		let revisions = rows
			.iter()
			.map(|row| -> Result<_, Error> {
				Ok((series_key(&row.0, row.1)?, row.8))
			})
			.collect::<Result<HashMap<_, _>, Error>>()?;
		let series = rows
			.into_iter()
			.map(|row| {
				series_from(StoredSeries {
					source: row.0,
					id: row.1,
					title: row.2,
					year: row.3,
					type_title: row.4,
					episode_count: row.5,
					my_anime_list_id: row.6,
					anilist_id: row.7,
				})
			})
			.collect::<Result<Vec<_>, _>>()?;
		let aliases = aliases
			.into_iter()
			.map(|(query, source, id)| Ok((query, series_key(&source, id)?)))
			.collect::<Result<BTreeMap<_, _>, Error>>()?;
		let refreshed_at = source_states
			.into_iter()
			.filter_map(|(source, _, refreshed_at)| {
				refreshed_at.map(|refreshed_at| (source, refreshed_at))
			})
			.map(|(source, refreshed_at)| {
				Ok((
					source_from_storage(&source)?,
					u64_from(refreshed_at, "refresh time")?,
				))
			})
			.collect::<Result<BTreeMap<_, _>, Error>>()?;
		Ok(LoadedCatalogue::new(
			Catalogue::from_parts(refreshed_at, series, aliases),
			revision,
			revisions,
		))
	}
}

fn series_from(series: StoredSeries) -> Result<Series, Error> {
	Ok(Series {
		source: source_from_storage(&series.source)?,
		id: u64_from(series.id, "Series ID")?,
		title: series.title,
		year: series
			.year
			.map(|year| u16::try_from(year).map_err(read_error))
			.transpose()?,
		type_title: series.type_title,
		number_of_episodes: series
			.episode_count
			.map(|count| u32::try_from(count).map_err(read_error))
			.transpose()?,
		my_anime_list_id: series
			.my_anime_list_id
			.map(|id| u64_from(id, "MyAnimeList ID"))
			.transpose()?,
		anilist_id: series
			.anilist_id
			.map(|id| u64_from(id, "AniList ID"))
			.transpose()?,
		poster_url_small: None,
		episodes: Vec::new(),
	})
}

type StoredRow = (
	String,
	i64,
	String,
	Option<i64>,
	Option<String>,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	i64,
);

fn series_key(source: &str, id: i64) -> Result<SeriesKey, Error> {
	Ok(SeriesKey::new(
		source_from_storage(source)?,
		u64_from(id, "Series ID")?,
	))
}

fn source_from_storage(source: &str) -> Result<ContentSource, Error> {
	ContentSource::from_storage(source).ok_or_else(|| {
		read_error(format!("cache contains unknown source {source:?}"))
	})
}
