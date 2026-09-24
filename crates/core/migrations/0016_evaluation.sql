-- A reviewer's note that the audio is a different version (mix, edit, live
-- take) from the one the sources described. Counted by the pilot report.
CREATE TABLE version_flag (
    track_id   TEXT PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
