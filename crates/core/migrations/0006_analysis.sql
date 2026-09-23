-- Fingerprints, one per analysed file, kept after the file is gone.
CREATE TABLE fingerprint (
    id            TEXT PRIMARY KEY,
    track_id      TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    audio_file_id TEXT REFERENCES audio_file(id) ON DELETE SET NULL,
    algorithm     TEXT NOT NULL,
    duration_ms   INTEGER NOT NULL,
    data          BLOB NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE (audio_file_id, algorithm)
);

CREATE INDEX fingerprint_track ON fingerprint(track_id);

-- Which segment a feature row describes; NULL for the whole-track summary.
ALTER TABLE feature_record ADD COLUMN segment_index INTEGER;
-- Extra analysis results (key name and confidence, tempo confidence,
-- quality) as JSON on the summary row.
ALTER TABLE feature_record ADD COLUMN details TEXT;

CREATE INDEX feature_record_track ON feature_record(track_id, model_id);

-- Analysis progress per track and feature version, so an upgrade that
-- cannot run (no audio) is visible and not retried in a loop.
CREATE TABLE analysis_state (
    track_id              TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    model_id              TEXT NOT NULL,
    weights_checksum      TEXT NOT NULL,
    preprocessing_version TEXT NOT NULL,
    state                 TEXT NOT NULL CHECK (state IN ('queued', 'done', 'needs_audio', 'failed')),
    reason                TEXT,
    updated_at            INTEGER NOT NULL,
    PRIMARY KEY (track_id, model_id, weights_checksum, preprocessing_version),
    CHECK (state IN ('queued', 'done') OR reason IS NOT NULL)
);
