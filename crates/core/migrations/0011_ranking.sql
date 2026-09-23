-- The review order from the last rerank, and why a candidate holds its
-- place (exploration picks say so).
ALTER TABLE candidate ADD COLUMN queue_rank INTEGER;
ALTER TABLE candidate ADD COLUMN rank_note TEXT;
CREATE INDEX candidate_queue_rank ON candidate(queue_rank);
