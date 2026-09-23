-- A link the user rejected stays recorded so a refresh cannot bring it back.
ALTER TABLE youtube_match ADD COLUMN rejected INTEGER NOT NULL DEFAULT 0;

-- Where each track's YouTube lookup stands, so it can be inspected.
CREATE TABLE youtube_lookup (
    track_id   TEXT PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE,
    status     TEXT NOT NULL CHECK (status IN ('queued', 'found', 'uncertain', 'none', 'waiting', 'failed')),
    detail     TEXT,
    updated_at INTEGER NOT NULL
);
