-- Playlists as discovery workspaces (#6): a brief, playlist seeds, the
-- playlists each candidate was suggested for, and fit feedback that is
-- kept apart from personal ratings.
ALTER TABLE playlist ADD COLUMN brief TEXT NOT NULL DEFAULT '';
-- Whether the app looks for tracks for this playlist in the background.
ALTER TABLE playlist ADD COLUMN discovery INTEGER NOT NULL DEFAULT 0;

CREATE TABLE playlist_seed (
    playlist_id TEXT NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('artist', 'label', 'dj', 'track')),
    value       TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, kind, value)
);

CREATE TABLE candidate_context (
    candidate_id TEXT NOT NULL REFERENCES candidate(id) ON DELETE CASCADE,
    playlist_id  TEXT NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (candidate_id, playlist_id)
);
CREATE INDEX candidate_context_playlist ON candidate_context(playlist_id);

-- Append-only. The latest event per playlist and track that is not undone
-- is the verdict. "fits" also added the track to the playlist when
-- added_to_playlist is 1, so undo can take it out again.
CREATE TABLE playlist_feedback (
    id                TEXT PRIMARY KEY,
    playlist_id       TEXT NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
    track_id          TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    verdict           TEXT NOT NULL CHECK (verdict IN ('fits', 'not_for_this')),
    added_to_playlist INTEGER NOT NULL DEFAULT 0,
    undone_at         INTEGER,
    created_at        INTEGER NOT NULL
);
CREATE INDEX playlist_feedback_track ON playlist_feedback(playlist_id, track_id);

ALTER TABLE source_run ADD COLUMN playlist_id TEXT REFERENCES playlist(id) ON DELETE SET NULL;
