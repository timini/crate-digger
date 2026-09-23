-- Order rating events by insertion (rowid) rather than timestamp, so two
-- events in the same millisecond still have a defined order.
DROP VIEW effective_rating;
CREATE VIEW effective_rating AS
SELECT track_id, kind, created_at FROM (
    SELECT e.track_id, e.kind, e.created_at,
           ROW_NUMBER() OVER (PARTITION BY e.track_id ORDER BY e.rowid DESC) AS rn
    FROM rating_event e
    WHERE e.kind IN ('thumbs_down', 'star1', 'star2', 'star3')
      AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = e.id)
) WHERE rn = 1;
