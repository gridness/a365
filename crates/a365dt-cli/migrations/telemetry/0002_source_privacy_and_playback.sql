ALTER TABLE command_events RENAME TO command_events_v1;

CREATE TABLE command_events (
	id INTEGER PRIMARY KEY,
	invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
	observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
	command TEXT NOT NULL CHECK (command IN (
		'cache_prune', 'completions', 'doctor', 'download', 'playback', 'stats',
		'telemetry_disable', 'telemetry_enable', 'telemetry_show', 'update'
	)),
	outcome TEXT NOT NULL CHECK (outcome IN (
		'success', 'failure', 'cancelled'
	))
) STRICT;

INSERT INTO command_events SELECT * FROM command_events_v1;
DROP TABLE command_events_v1;

ALTER TABLE series_selection_events RENAME TO series_selection_events_v1;

CREATE TABLE series_selection_events (
	id INTEGER PRIMARY KEY,
	invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
	observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
	series_source TEXT CHECK (series_source IN ('anime365', 'h365')),
	series_id INTEGER CHECK (series_id > 0),
	series_title TEXT CHECK (series_title <> ''),
	identity_redacted INTEGER NOT NULL CHECK (identity_redacted IN (0, 1)),
	catalogue_result TEXT CHECK (catalogue_result IN ('hit', 'miss')),
	CHECK (
		(identity_redacted = 1 AND series_source IS NULL
			AND series_id IS NULL AND series_title IS NULL)
		OR
		(identity_redacted = 0 AND series_source IS NOT NULL
			AND series_id IS NOT NULL AND series_title IS NOT NULL)
	)
) STRICT;

INSERT INTO series_selection_events (
	id, invocation_id, observed_at_ms, series_source, series_id, series_title,
	identity_redacted, catalogue_result
)
SELECT id, invocation_id, observed_at_ms, 'anime365', series_id, series_title,
	0, catalogue_result
FROM series_selection_events_v1;
DROP TABLE series_selection_events_v1;

ALTER TABLE download_outcomes RENAME TO download_outcomes_v1;
ALTER TABLE download_batches RENAME TO download_batches_v1;

CREATE TABLE download_batches (
	id INTEGER PRIMARY KEY,
	invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
	observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
	series_source TEXT CHECK (series_source IN ('anime365', 'h365')),
	series_id INTEGER CHECK (series_id > 0),
	series_title TEXT CHECK (series_title <> ''),
	identity_redacted INTEGER NOT NULL CHECK (identity_redacted IN (0, 1)),
	duration_us INTEGER NOT NULL CHECK (duration_us >= 0),
	CHECK (
		(identity_redacted = 1 AND series_source IS NULL
			AND series_id IS NULL AND series_title IS NULL)
		OR
		(identity_redacted = 0 AND series_source IS NOT NULL
			AND series_id IS NOT NULL AND series_title IS NOT NULL)
	)
) STRICT;

INSERT INTO download_batches (
	id, invocation_id, observed_at_ms, series_source, series_id, series_title,
	identity_redacted, duration_us
)
SELECT id, invocation_id, observed_at_ms, 'anime365', series_id, series_title,
	0, duration_us
FROM download_batches_v1;

CREATE TABLE download_outcomes (
	id INTEGER PRIMARY KEY,
	batch_id INTEGER NOT NULL
		REFERENCES download_batches(id) ON DELETE CASCADE,
	status TEXT NOT NULL CHECK (status IN (
		'downloaded', 'skipped', 'failed', 'mux_failed', 'interrupted'
	)),
	downloaded_bytes INTEGER CHECK (downloaded_bytes >= 0),
	CHECK (
		(status = 'downloaded') = (downloaded_bytes IS NOT NULL)
	)
) STRICT;

INSERT INTO download_outcomes SELECT * FROM download_outcomes_v1;
DROP TABLE download_outcomes_v1;
DROP TABLE download_batches_v1;

CREATE TABLE playback_sessions (
	id INTEGER PRIMARY KEY,
	invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
	observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
	series_source TEXT CHECK (series_source IN ('anime365', 'h365')),
	series_id INTEGER CHECK (series_id > 0),
	series_title TEXT CHECK (series_title <> ''),
	identity_redacted INTEGER NOT NULL CHECK (identity_redacted IN (0, 1)),
	duration_us INTEGER NOT NULL CHECK (duration_us >= 0),
	outcome TEXT NOT NULL CHECK (outcome IN (
		'failure', 'interrupted', 'natural_end', 'stopped'
	)),
	CHECK (
		(identity_redacted = 1 AND series_source IS NULL
			AND series_id IS NULL AND series_title IS NULL)
		OR
		(identity_redacted = 0 AND series_source IS NOT NULL
			AND series_id IS NOT NULL AND series_title IS NOT NULL)
	)
) STRICT;

CREATE INDEX command_events_by_time
	ON command_events(observed_at_ms, id);
CREATE INDEX command_events_by_invocation
	ON command_events(invocation_id, observed_at_ms, id);
CREATE INDEX series_selection_events_by_time
	ON series_selection_events(observed_at_ms, id);
CREATE INDEX series_selection_events_by_invocation
	ON series_selection_events(invocation_id, observed_at_ms, id);
CREATE INDEX download_batches_by_time
	ON download_batches(observed_at_ms, id);
CREATE INDEX download_batches_by_invocation
	ON download_batches(invocation_id, observed_at_ms, id);
CREATE INDEX download_outcomes_by_batch
	ON download_outcomes(batch_id);
CREATE INDEX playback_sessions_by_time
	ON playback_sessions(observed_at_ms, id);
CREATE INDEX playback_sessions_by_invocation
	ON playback_sessions(invocation_id, observed_at_ms, id);
