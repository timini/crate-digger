-- Text the user pasted for discovery. Evidence refers to it by id.
CREATE TABLE supplied_text (
    id         TEXT PRIMARY KEY,
    label      TEXT,
    text       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- One row per discovery attempt, so a failed retrieval is visible and
-- distinct from a run that found nothing new.
CREATE TABLE source_run (
    id            TEXT PRIMARY KEY,
    source        TEXT NOT NULL,
    input         TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER NOT NULL,
    outcome       TEXT NOT NULL CHECK (outcome IN ('found', 'empty', 'failed')),
    created       INTEGER NOT NULL DEFAULT 0,
    already_known INTEGER NOT NULL DEFAULT 0,
    unverified    INTEGER NOT NULL DEFAULT 0,
    detail        TEXT
);

CREATE INDEX source_run_finished ON source_run(finished_at);
