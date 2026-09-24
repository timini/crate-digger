-- The library map (#25). One saved map at a time; rebuilding replaces it.
CREATE TABLE similarity_map (
    id                    TEXT PRIMARY KEY,
    model_id              TEXT NOT NULL,
    weights_checksum      TEXT NOT NULL,
    preprocessing_version TEXT NOT NULL,
    pipeline_version      TEXT NOT NULL,
    params                TEXT NOT NULL, -- JSON, including the eps actually used
    placed                INTEGER NOT NULL,
    clusters              INTEGER NOT NULL,
    noise                 INTEGER NOT NULL,
    newest_feature_at     INTEGER NOT NULL,
    build_ms              INTEGER NOT NULL,
    created_at            INTEGER NOT NULL
);

CREATE TABLE similarity_point (
    map_id   TEXT NOT NULL REFERENCES similarity_map(id) ON DELETE CASCADE,
    track_id TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    x        REAL NOT NULL,
    y        REAL NOT NULL,
    cluster  INTEGER, -- NULL is DBSCAN noise
    PRIMARY KEY (map_id, track_id)
);

CREATE TABLE similarity_edge (
    map_id       TEXT NOT NULL REFERENCES similarity_map(id) ON DELETE CASCADE,
    track_id     TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    neighbour_id TEXT NOT NULL REFERENCES track(id) ON DELETE CASCADE,
    rank         INTEGER NOT NULL,
    distance     REAL NOT NULL,
    PRIMARY KEY (map_id, track_id, rank)
);
