//! Possible duplicate tracks and how the user resolves them.
//!
//! Byte-identical copies are attached to one track automatically during
//! import. Everything else (same artist, title and mix with a similar
//! duration) is only a suggestion: title similarity is not identity, so two
//! tracks are merged only when the user confirms, and the confirmation is
//! recorded as the merge evidence.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::util::now_ms;
use crate::{meta, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicatePair {
    pub track_a: String,
    pub track_b: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub duration_a_ms: Option<i64>,
    pub duration_b_ms: Option<i64>,
    pub path_a: Option<String>,
    pub path_b: Option<String>,
}

/// Track pairs with the same artist, title and mix (ignoring case) whose
/// primary files are within two seconds of each other, excluding pairs the
/// user has dismissed.
pub fn suggestions(conn: &Connection, limit: i64) -> Result<Vec<DuplicatePair>> {
    let mut stmt = conn.prepare(
        "WITH lib AS (
             SELECT m.track_id, m.artist, m.title, m.mix,
                    (SELECT duration_ms FROM audio_file f WHERE f.track_id = m.track_id
                     ORDER BY f.is_primary DESC LIMIT 1) AS dur,
                    (SELECT path FROM audio_file f WHERE f.track_id = m.track_id
                     ORDER BY f.is_primary DESC LIMIT 1) AS path
             FROM track_meta m
             WHERE m.artist IS NOT NULL AND m.title IS NOT NULL
               AND EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = m.track_id)
         )
         SELECT a.track_id, b.track_id, a.artist, a.title, a.mix, a.dur, b.dur, a.path, b.path
         FROM lib a JOIN lib b
           ON a.artist = b.artist COLLATE NOCASE
          AND a.title = b.title COLLATE NOCASE
          AND COALESCE(a.mix, '') = COALESCE(b.mix, '') COLLATE NOCASE
          AND a.track_id < b.track_id
         WHERE (a.dur IS NULL OR b.dur IS NULL OR ABS(a.dur - b.dur) <= 2000)
           AND NOT EXISTS (SELECT 1 FROM duplicate_dismissal d
                           WHERE d.track_a = a.track_id AND d.track_b = b.track_id)
         ORDER BY a.artist COLLATE NOCASE, a.title COLLATE NOCASE
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(DuplicatePair {
                track_a: r.get(0)?,
                track_b: r.get(1)?,
                artist: r.get(2)?,
                title: r.get(3)?,
                mix: r.get(4)?,
                duration_a_ms: r.get(5)?,
                duration_b_ms: r.get(6)?,
                path_a: r.get(7)?,
                path_b: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Record that two tracks are different recordings.
pub fn dismiss(conn: &Connection, a: &str, b: &str) -> Result<()> {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    conn.execute(
        "INSERT OR IGNORE INTO duplicate_dismissal (track_a, track_b, created_at) VALUES (?1, ?2, ?3)",
        params![a, b, now_ms()],
    )?;
    Ok(())
}

/// Merge `remove` into `keep`. Files, ratings, playlist entries, features,
/// links and metadata move to `keep`; where both have a value, `keep`'s
/// wins. `evidence` is stored with the redirect from the old ID.
pub fn merge(conn: &Connection, keep: &str, remove: &str, evidence: &str) -> Result<()> {
    if keep == remove {
        return Err(Error::Invalid("cannot merge a track into itself".into()));
    }
    if evidence.trim().is_empty() {
        return Err(Error::Invalid("a merge needs recorded evidence".into()));
    }
    for id in [keep, remove] {
        conn.query_row("SELECT 1 FROM track WHERE id = ?1", params![id], |_| Ok(()))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("track {id}")))?;
    }
    let both_candidates: i64 = conn.query_row(
        "SELECT COUNT(*) FROM candidate WHERE track_id IN (?1, ?2)",
        params![keep, remove],
        |r| r.get(0),
    )?;
    if both_candidates == 2 {
        return Err(Error::Conflict(
            "both tracks are discovery candidates; resolve one of them in Review first".into(),
        ));
    }

    let tx = conn.unchecked_transaction()?;
    let p = params![keep, remove];
    tx.execute(
        "UPDATE audio_file SET track_id = ?1, is_primary = 0 WHERE track_id = ?2",
        p,
    )?;
    tx.execute("UPDATE rating_event SET track_id = ?1 WHERE track_id = ?2", p)?;
    tx.execute("UPDATE playlist_entry SET track_id = ?1 WHERE track_id = ?2", p)?;
    tx.execute("UPDATE feature_record SET track_id = ?1 WHERE track_id = ?2", p)?;
    tx.execute("UPDATE candidate SET track_id = ?1 WHERE track_id = ?2", p)?;
    for table in [
        "keep_decision",
        "field_value",
        "field_correction",
        "youtube_match",
        "track_external_id",
        "track_release",
    ] {
        tx.execute(
            &format!("UPDATE OR IGNORE {table} SET track_id = ?1 WHERE track_id = ?2"),
            p,
        )?;
        tx.execute(
            &format!("DELETE FROM {table} WHERE track_id = ?1"),
            params![remove],
        )?;
    }
    tx.execute(
        "DELETE FROM duplicate_dismissal WHERE track_a = ?1 OR track_b = ?1",
        params![remove],
    )?;
    tx.execute("UPDATE track_redirect SET new_id = ?1 WHERE new_id = ?2", p)?;
    tx.execute("DELETE FROM track_meta WHERE track_id = ?1", params![remove])?;
    tx.execute("DELETE FROM track WHERE id = ?1", params![remove])?;
    tx.execute(
        "INSERT INTO track_redirect (old_id, new_id, evidence, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![remove, keep, evidence, now_ms()],
    )?;
    // Keep exactly one primary file.
    let has_primary: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM audio_file WHERE track_id = ?1 AND is_primary = 1)",
        params![keep],
        |r| r.get(0),
    )?;
    if !has_primary {
        tx.execute(
            "UPDATE audio_file SET is_primary = 1 WHERE id =
                 (SELECT id FROM audio_file WHERE track_id = ?1 ORDER BY created_at LIMIT 1)",
            params![keep],
        )?;
    }
    meta::refresh_effective(&tx, keep)?;
    tx.commit()?;
    Ok(())
}

/// Follow merge redirects to the current track ID.
pub fn resolve_track_id(conn: &Connection, id: &str) -> Result<String> {
    let mut current = id.to_string();
    for _ in 0..16 {
        match conn
            .query_row(
                "SELECT new_id FROM track_redirect WHERE old_id = ?1",
                params![current],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            Some(next) => current = next,
            None => return Ok(current),
        }
    }
    Err(Error::Invalid(format!("redirect loop at track {id}")))
}
