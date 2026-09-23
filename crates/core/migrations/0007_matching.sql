-- Lookups for identity matching. Both are filled in by Rust code (see
-- db::after_migration), because folding names and decoding fingerprints
-- cannot be done in SQL.

-- Folded title (accents, case, punctuation and featured artists removed).
ALTER TABLE track_meta ADD COLUMN title_key TEXT;
CREATE INDEX track_meta_title_key ON track_meta(title_key);

-- A sample of each fingerprint's values, to find tracks that share audio
-- without comparing against every fingerprint.
CREATE TABLE fingerprint_key (
    key            INTEGER NOT NULL,
    fingerprint_id TEXT NOT NULL REFERENCES fingerprint(id) ON DELETE CASCADE
);
CREATE INDEX fingerprint_key_key ON fingerprint_key(key);
CREATE INDEX fingerprint_key_fingerprint ON fingerprint_key(fingerprint_id);
