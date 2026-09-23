-- Canonical identity: works group the versions of one composition,
-- evidence records why two tracks are (or are not) the same recording, and
-- conflicts wait for the user to decide.

CREATE TABLE work (
    id         TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);

ALTER TABLE track ADD COLUMN work_id TEXT REFERENCES work(id);
CREATE INDEX track_work ON track(work_id);

-- Another spelling of an artist, as normalised keys.
CREATE TABLE artist_alias (
    alias_key     TEXT PRIMARY KEY,
    canonical_key TEXT NOT NULL,
    source        TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE TABLE identity_evidence (
    id         TEXT PRIMARY KEY,
    track_a    TEXT NOT NULL,
    track_b    TEXT NOT NULL,
    kind       TEXT NOT NULL,
    verdict    TEXT NOT NULL,
    detail     TEXT NOT NULL, -- JSON
    source     TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX identity_evidence_tracks ON identity_evidence(track_a, track_b);

CREATE TABLE identity_conflict (
    id          TEXT PRIMARY KEY,
    track_a     TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    track_b     TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    reason      TEXT NOT NULL,
    evidence    TEXT NOT NULL, -- JSON list
    state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'resolved')),
    resolution  TEXT CHECK (resolution IN ('same_recording', 'different_version', 'unrelated')),
    created_at  INTEGER NOT NULL,
    resolved_at INTEGER,
    CHECK (track_a < track_b),
    CHECK ((state = 'resolved') = (resolution IS NOT NULL)),
    UNIQUE (track_a, track_b)
);

-- How a file relates to its track's recording: NULL for a normal copy,
-- otherwise for example 'pitched:+4.0'.
ALTER TABLE audio_file ADD COLUMN variant TEXT;
