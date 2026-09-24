-- Allow clearing a rating. The log stays append-only: a 'cleared' event
-- becomes the latest rating event, and the effective rating reads it as
-- unrated. SQLite cannot change a CHECK constraint, so the table is rebuilt
-- with every event kept.
DROP VIEW effective_rating;

CREATE TABLE rating_event_new (
    id              TEXT PRIMARY KEY,
    track_id        TEXT NOT NULL REFERENCES track(id),
    kind            TEXT NOT NULL CHECK (kind IN (
                        'thumbs_down', 'star1', 'star2', 'star3', 'cleared', 'skip', 'undo')),
    session_id      TEXT NOT NULL,
    undoes_event_id TEXT REFERENCES rating_event_new(id),
    created_at      INTEGER NOT NULL,
    CHECK ((kind = 'undo') = (undoes_event_id IS NOT NULL))
);

-- Keep insertion order, which the effective rating relies on.
INSERT INTO rating_event_new (rowid, id, track_id, kind, session_id, undoes_event_id, created_at)
SELECT rowid, id, track_id, kind, session_id, undoes_event_id, created_at FROM rating_event ORDER BY rowid;

DROP TABLE rating_event;
ALTER TABLE rating_event_new RENAME TO rating_event;

CREATE INDEX rating_event_track ON rating_event(track_id, created_at);
CREATE INDEX rating_event_session ON rating_event(session_id, created_at);
CREATE INDEX rating_event_undoes ON rating_event(undoes_event_id);

CREATE VIEW effective_rating AS
SELECT track_id, kind, created_at FROM (
    SELECT e.track_id, e.kind, e.created_at,
           ROW_NUMBER() OVER (PARTITION BY e.track_id ORDER BY e.rowid DESC) AS rn
    FROM rating_event e
    WHERE e.kind IN ('thumbs_down', 'star1', 'star2', 'star3', 'cleared')
      AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = e.id)
) WHERE rn = 1 AND kind != 'cleared';
