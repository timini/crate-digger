-- Crate Digger initial schema.
-- Timestamps are Unix milliseconds. IDs are UUIDv7 strings.
-- Track identity never depends on a file path: paths live only on audio_file.

CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL -- JSON
);

CREATE TABLE library_root (
    id         TEXT PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

-- A recording or specific mix.
CREATE TABLE track (
    id         TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- When two track records are merged, the old ID redirects to the survivor.
CREATE TABLE track_redirect (
    old_id     TEXT PRIMARY KEY,
    new_id     TEXT NOT NULL REFERENCES track(id),
    evidence   TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE track_external_id (
    track_id  TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    namespace TEXT NOT NULL, -- e.g. discogs_release, isrc, musicbrainz_recording
    value     TEXT NOT NULL,
    source    TEXT NOT NULL,
    PRIMARY KEY (namespace, value, track_id)
);

CREATE TABLE release (
    id             TEXT PRIMARY KEY,
    title          TEXT NOT NULL,
    label          TEXT,
    catalog_number TEXT,
    year           INTEGER,
    created_at     INTEGER NOT NULL
);

CREATE TABLE track_release (
    track_id     TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    release_id   TEXT NOT NULL REFERENCES release(id) ON DELETE CASCADE,
    track_number TEXT,
    source       TEXT NOT NULL,
    PRIMARY KEY (track_id, release_id)
);

-- Automatically extracted metadata, one row per (track, field, source).
CREATE TABLE field_value (
    track_id   TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    field      TEXT NOT NULL,
    source     TEXT NOT NULL, -- tags, fake_source, discogs, analysis ...
    value      TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (track_id, field, source)
);

-- User corrections. Always take precedence over field_value.
CREATE TABLE field_correction (
    track_id   TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    field      TEXT NOT NULL,
    value      TEXT NOT NULL, -- empty string means "user cleared this field"
    created_at INTEGER NOT NULL,
    PRIMARY KEY (track_id, field)
);

-- Effective metadata, rebuilt in Rust whenever values or corrections change.
CREATE TABLE track_meta (
    track_id     TEXT PRIMARY KEY REFERENCES track(id) ON DELETE CASCADE,
    artist       TEXT,
    title        TEXT,
    mix          TEXT,
    label        TEXT,
    release      TEXT,
    track_number TEXT,
    year         INTEGER,
    genre        TEXT,
    tempo        REAL,
    musical_key  TEXT
);

CREATE INDEX track_meta_tempo ON track_meta(tempo);
CREATE INDEX track_meta_key ON track_meta(musical_key);

CREATE VIRTUAL TABLE track_fts USING fts5(
    artist, title, mix, label, release,
    content = 'track_meta',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER track_meta_ai AFTER INSERT ON track_meta BEGIN
    INSERT INTO track_fts(rowid, artist, title, mix, label, release)
    VALUES (new.rowid, new.artist, new.title, new.mix, new.label, new.release);
END;
CREATE TRIGGER track_meta_ad AFTER DELETE ON track_meta BEGIN
    INSERT INTO track_fts(track_fts, rowid, artist, title, mix, label, release)
    VALUES ('delete', old.rowid, old.artist, old.title, old.mix, old.label, old.release);
END;
CREATE TRIGGER track_meta_au AFTER UPDATE ON track_meta BEGIN
    INSERT INTO track_fts(track_fts, rowid, artist, title, mix, label, release)
    VALUES ('delete', old.rowid, old.artist, old.title, old.mix, old.label, old.release);
    INSERT INTO track_fts(rowid, artist, title, mix, label, release)
    VALUES (new.rowid, new.artist, new.title, new.mix, new.label, new.release);
END;

-- A concrete file on disk. The only table that stores paths.
CREATE TABLE audio_file (
    id                  TEXT PRIMARY KEY,
    track_id            TEXT NOT NULL REFERENCES track(id),
    path                TEXT NOT NULL UNIQUE,
    origin              TEXT NOT NULL CHECK (origin IN ('imported', 'staged', 'archived')),
    library_root_id     TEXT REFERENCES library_root(id) ON DELETE SET NULL,
    size_bytes          INTEGER NOT NULL,
    mtime_ms            INTEGER NOT NULL,
    content_hash        TEXT NOT NULL,
    duration_ms         INTEGER,
    format              TEXT,
    sample_rate         INTEGER,
    channels            INTEGER,
    bitrate_kbps        INTEGER,
    availability        TEXT NOT NULL DEFAULT 'available'
                        CHECK (availability IN ('available', 'missing', 'corrupt')),
    availability_reason TEXT,
    is_primary          INTEGER NOT NULL DEFAULT 0,
    waveform            BLOB,
    last_checked_at     INTEGER NOT NULL,
    created_at          INTEGER NOT NULL,
    CHECK (availability = 'available' OR availability_reason IS NOT NULL)
);

CREATE INDEX audio_file_track ON audio_file(track_id);
CREATE INDEX audio_file_hash ON audio_file(content_hash);

-- Discovery candidates and their pipeline position.
CREATE TABLE candidate (
    id            TEXT PRIMARY KEY,
    track_id      TEXT NOT NULL UNIQUE REFERENCES track(id),
    stage         TEXT NOT NULL CHECK (stage IN (
                      'candidate', 'identified', 'acquisition_queued', 'downloading',
                      'validating', 'analysing', 'ready', 'reviewed')),
    status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active', 'paused', 'blocked', 'failed', 'cancelled')),
    status_reason TEXT,
    verified      INTEGER NOT NULL DEFAULT 0, -- supported by retrievable evidence
    confidence    REAL,
    score         REAL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    CHECK (status = 'active' OR status_reason IS NOT NULL)
);

CREATE INDEX candidate_stage ON candidate(stage, status);

CREATE TABLE evidence (
    id               TEXT PRIMARY KEY,
    candidate_id     TEXT NOT NULL REFERENCES candidate(id) ON DELETE CASCADE,
    source_kind      TEXT NOT NULL,
    source_url       TEXT,
    supplied_text_id TEXT,
    retrieved_at     INTEGER NOT NULL,
    excerpt          TEXT NOT NULL,
    confidence       REAL NOT NULL,
    CHECK (source_url IS NOT NULL OR supplied_text_id IS NOT NULL)
);

CREATE TABLE explanation (
    id           TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL REFERENCES candidate(id) ON DELETE CASCADE,
    reason       TEXT NOT NULL,
    weight       REAL NOT NULL DEFAULT 0
);

CREATE TABLE seed (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('artist', 'label', 'dj', 'track')),
    value      TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (kind, value)
);

CREATE TABLE youtube_match (
    id             TEXT PRIMARY KEY,
    track_id       TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    video_id       TEXT NOT NULL,
    url            TEXT NOT NULL,
    title          TEXT,
    channel        TEXT,
    duration_ms    INTEGER,
    looked_up_at   INTEGER NOT NULL,
    confidence     REAL NOT NULL,
    preferred      INTEGER NOT NULL DEFAULT 0,
    user_corrected INTEGER NOT NULL DEFAULT 0,
    UNIQUE (track_id, video_id)
);

-- Features survive deletion of the audio they came from.
CREATE TABLE feature_record (
    id                    TEXT PRIMARY KEY,
    track_id              TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    audio_file_id         TEXT REFERENCES audio_file(id) ON DELETE SET NULL,
    model_id              TEXT NOT NULL,
    weights_checksum      TEXT NOT NULL,
    preprocessing_version TEXT NOT NULL,
    source_fingerprint    TEXT NOT NULL,
    segment_start_ms      INTEGER NOT NULL,
    segment_end_ms        INTEGER NOT NULL,
    dims                  INTEGER NOT NULL,
    embedding             BLOB,
    tempo                 REAL,
    musical_key           TEXT,
    loudness_lufs         REAL,
    quality               REAL,
    created_at            INTEGER NOT NULL
);

-- Append-only preference log. The effective rating is derived by replay.
CREATE TABLE rating_event (
    id              TEXT PRIMARY KEY,
    track_id        TEXT NOT NULL REFERENCES track(id),
    kind            TEXT NOT NULL CHECK (kind IN (
                        'thumbs_down', 'star1', 'star2', 'star3', 'skip', 'undo')),
    session_id      TEXT NOT NULL,
    undoes_event_id TEXT REFERENCES rating_event(id),
    created_at      INTEGER NOT NULL,
    CHECK ((kind = 'undo') = (undoes_event_id IS NOT NULL))
);

CREATE INDEX rating_event_track ON rating_event(track_id, created_at);
CREATE INDEX rating_event_session ON rating_event(session_id, created_at);

-- Archival decisions, independent of ratings and playlists.
CREATE TABLE keep_decision (
    track_id   TEXT PRIMARY KEY REFERENCES track(id),
    decided_at INTEGER NOT NULL
);

CREATE TABLE playlist (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE playlist_entry (
    playlist_id TEXT NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    TEXT NOT NULL REFERENCES track(id),
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position)
);

CREATE INDEX playlist_entry_track ON playlist_entry(track_id);

-- Durable background work.
CREATE TABLE job (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    connector        TEXT,
    state            TEXT NOT NULL CHECK (state IN (
                         'queued', 'running', 'paused', 'blocked', 'failed', 'cancelled', 'done')),
    reason           TEXT,
    payload          TEXT NOT NULL, -- JSON
    checkpoint       TEXT,          -- JSON, progress that must not be repeated
    idempotency_key  TEXT NOT NULL UNIQUE,
    attempts         INTEGER NOT NULL DEFAULT 0,
    max_attempts     INTEGER NOT NULL DEFAULT 5,
    next_run_at      INTEGER NOT NULL,
    lease_owner      TEXT,
    lease_expires_at INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    CHECK (state IN ('queued', 'running', 'done') OR reason IS NOT NULL)
);

CREATE INDEX job_claim ON job(state, kind, next_run_at);

CREATE TABLE connector_state (
    connector  TEXT PRIMARY KEY,
    status     TEXT NOT NULL CHECK (status IN ('ok', 'auth_failed', 'unavailable')),
    reason     TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE daily_counter (
    day   TEXT NOT NULL, -- YYYY-MM-DD, UTC
    kind  TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (day, kind)
);

-- Journal for moving files into the archive, so interrupted moves recover.
CREATE TABLE archive_op (
    id            TEXT PRIMARY KEY,
    audio_file_id TEXT NOT NULL REFERENCES audio_file(id),
    src_path      TEXT NOT NULL,
    dest_path     TEXT NOT NULL,
    step          TEXT NOT NULL CHECK (step IN (
                      'intent', 'copied', 'db_updated', 'done', 'rolled_back')),
    error         TEXT,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

-- Central-service records (used from milestone 4).
CREATE TABLE sync_outbox (
    id              TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    kind            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL CHECK (state IN ('pending', 'acked', 'rejected')),
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    acked_at        INTEGER
);

CREATE TABLE contribution (
    id              TEXT PRIMARY KEY,
    outbox_id       TEXT REFERENCES sync_outbox(id),
    track_id        TEXT NOT NULL REFERENCES track(id),
    kind            TEXT NOT NULL CHECK (kind IN ('metadata', 'feature', 'correction')),
    created_at      INTEGER NOT NULL
);

CREATE TABLE backup_operation (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN ('upload', 'restore', 'delete')),
    idempotency_key TEXT NOT NULL UNIQUE,
    snapshot_id     TEXT,
    state           TEXT NOT NULL CHECK (state IN ('pending', 'done', 'failed')),
    error           TEXT,
    created_at      INTEGER NOT NULL,
    completed_at    INTEGER
);
