-- Where each track's metadata lookup stands.
CREATE TABLE metadata_lookup (
    track_id   TEXT PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE,
    status     TEXT NOT NULL CHECK (status IN ('queued', 'identified', 'suggested', 'not_found', 'waiting', 'failed')),
    detail     TEXT,
    updated_at INTEGER NOT NULL
);

-- Lookup results not confident enough to apply; the user can accept one.
CREATE TABLE metadata_suggestion (
    id         TEXT PRIMARY KEY,
    track_id   TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    score      REAL NOT NULL,
    payload    TEXT NOT NULL, -- JSON: adapters::Identified
    created_at INTEGER NOT NULL
);
CREATE INDEX metadata_suggestion_track ON metadata_suggestion(track_id);
