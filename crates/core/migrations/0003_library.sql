-- Effective rating per track: the most recent rating that has not been
-- undone. Skips are not ratings and never appear here.
CREATE VIEW effective_rating AS
SELECT track_id, kind, created_at FROM (
    SELECT e.track_id, e.kind, e.created_at,
           ROW_NUMBER() OVER (PARTITION BY e.track_id ORDER BY e.created_at DESC, e.id DESC) AS rn
    FROM rating_event e
    WHERE e.kind IN ('thumbs_down', 'star1', 'star2', 'star3')
      AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = e.id)
) WHERE rn = 1;

CREATE INDEX rating_event_undoes ON rating_event(undoes_event_id);

-- Pairs of tracks the user said are not duplicates, so they are not
-- suggested again. Stored with track_a < track_b.
CREATE TABLE duplicate_dismissal (
    track_a    TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    track_b    TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (track_a, track_b),
    CHECK (track_a < track_b)
);

CREATE INDEX track_meta_artist_title ON track_meta(artist COLLATE NOCASE, title COLLATE NOCASE);
CREATE INDEX audio_file_origin ON audio_file(origin, availability);
