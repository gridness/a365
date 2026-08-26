DROP INDEX aliases_by_series;
DROP INDEX series_by_refresh;

ALTER TABLE aliases RENAME TO aliases_legacy;
ALTER TABLE series RENAME TO series_legacy;

CREATE TABLE series (
	source TEXT NOT NULL CHECK (source IN ('anime365', 'h365')),
	id INTEGER NOT NULL CHECK (id > 0),
	title TEXT NOT NULL CHECK (title <> ''),
	year INTEGER CHECK (year BETWEEN 0 AND 65535),
	type_title TEXT,
	episode_count INTEGER CHECK (
		episode_count BETWEEN 0 AND 4294967295
	),
	my_anime_list_id INTEGER CHECK (my_anime_list_id > 0),
	anilist_id INTEGER CHECK (anilist_id > 0),
	revision INTEGER NOT NULL CHECK (revision >= 0),
	refresh_generation INTEGER CHECK (refresh_generation >= 0),
	refresh_position INTEGER CHECK (refresh_position >= 0),
	discovery_order INTEGER NOT NULL CHECK (discovery_order >= 0),
	PRIMARY KEY (source, id),
	CHECK (
		(refresh_generation IS NULL) = (refresh_position IS NULL)
	)
) STRICT, WITHOUT ROWID;

INSERT INTO series (
	source, id, title, year, type_title, episode_count, my_anime_list_id,
	anilist_id, revision, refresh_generation, refresh_position,
	discovery_order
)
SELECT
	'anime365', id, title, year, type_title, episode_count, NULL, NULL, revision,
	refresh_generation, refresh_position, discovery_order
FROM series_legacy;

CREATE TABLE aliases (
	query TEXT PRIMARY KEY CHECK (query <> ''),
	series_source TEXT NOT NULL,
	series_id INTEGER NOT NULL,
	FOREIGN KEY (series_source, series_id)
		REFERENCES series(source, id) ON DELETE CASCADE
) STRICT;

INSERT INTO aliases (query, series_source, series_id)
SELECT query, 'anime365', series_id FROM aliases_legacy;

DROP TABLE aliases_legacy;
DROP TABLE series_legacy;

CREATE INDEX aliases_by_series
	ON aliases(series_source, series_id);

CREATE INDEX series_by_refresh
	ON series(refresh_generation, refresh_position, source, id)
	WHERE refresh_generation IS NOT NULL;

CREATE TABLE catalogue_source_state (
	source TEXT PRIMARY KEY CHECK (source IN ('anime365', 'h365')),
	current_generation INTEGER NOT NULL CHECK (current_generation >= 0),
	last_refresh_revision INTEGER NOT NULL CHECK (last_refresh_revision >= 0),
	refreshed_at INTEGER CHECK (refreshed_at >= 0)
) STRICT, WITHOUT ROWID;

INSERT INTO catalogue_source_state (
	source, current_generation, last_refresh_revision, refreshed_at
)
SELECT
	'anime365', current_generation, last_refresh_revision, refreshed_at
FROM catalogue_state;

INSERT INTO catalogue_source_state VALUES ('h365', 0, 0, NULL);
