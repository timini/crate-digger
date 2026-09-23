-- Search results waiting for the user to choose a download, because the
-- automatic rule was not met or unattended downloads are off.
CREATE TABLE acquisition_choice (
    candidate_id TEXT PRIMARY KEY REFERENCES candidate(id) ON DELETE CASCADE,
    connector    TEXT NOT NULL,
    outcome      TEXT NOT NULL, -- JSON: the ranked results and the rule's decision
    why          TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    resolved_at  INTEGER
);
