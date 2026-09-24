//! Identifying library tracks by audio fingerprint and filling in their
//! metadata, as background jobs.
//!
//! A confident match (AcoustID score of at least 0.9 and a matching length)
//! is applied as source `musicbrainz`, which ranks above file tags; extras
//! from Discogs go in as source `discogs`. Anything less confident is kept
//! as a suggestion for the user to accept. The user's own edits always win,
//! and the audio files are never changed.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{AdapterError, Identified, MetadataLookup, MetadataQuery};
use crate::domain::Field;
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::util::new_id;
use crate::{meta, Error, Result};

pub const CONNECTOR: &str = "acoustid";
/// Matches at or above this AcoustID score are applied without asking.
pub const CONFIDENT_SCORE: f64 = 0.9;
/// And only if the identified recording's length is this close.
pub const LENGTH_TOLERANCE_MS: i64 = 3_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupStatus {
    /// `queued`, `identified`, `suggested`, `not_found`, `waiting` or `failed`.
    pub status: String,
    pub detail: Option<String>,
    pub updated_at: i64,
}

fn set_status(conn: &Connection, track_id: &str, status: &str, detail: Option<&str>, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO metadata_lookup (track_id, status, detail, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET status = ?2, detail = ?3, updated_at = ?4",
        params![track_id, status, detail, now],
    )?;
    Ok(())
}

pub fn status(conn: &Connection, track_id: &str) -> Result<Option<LookupStatus>> {
    Ok(conn
        .query_row(
            "SELECT status, detail, updated_at FROM metadata_lookup WHERE track_id = ?1",
            params![track_id],
            |r| {
                Ok(LookupStatus {
                    status: r.get(0)?,
                    detail: r.get(1)?,
                    updated_at: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// Queue a lookup for one track. `again` starts a new one even if it ran before.
pub fn queue(conn: &Connection, track_id: &str, again: bool, now: i64) -> Result<bool> {
    let key = if again {
        format!("metadata:{track_id}:{}", new_id())
    } else {
        format!("metadata:{track_id}")
    };
    let job = crate::jobs::enqueue(
        conn,
        &NewJob::new(kinds::METADATA, key, serde_json::json!({ "track_id": track_id })).connector(CONNECTOR),
        now,
    )?;
    if job.created {
        set_status(conn, track_id, "queued", None, now)?;
    }
    Ok(job.created)
}

/// Queue every fingerprinted library track that has not been looked up.
pub fn queue_all(conn: &Connection, now: i64) -> Result<usize> {
    let tracks: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT f.track_id FROM fingerprint f
             WHERE NOT EXISTS (SELECT 1 FROM metadata_lookup m WHERE m.track_id = f.track_id)",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut queued = 0;
    for t in tracks {
        if queue(conn, &t, false, now)? {
            queued += 1;
        }
    }
    Ok(queued)
}

fn apply(conn: &Connection, track_id: &str, found: &Identified) -> Result<()> {
    let to_fields = |pairs: &[(String, String)]| -> Vec<(Field, Option<String>)> {
        pairs
            .iter()
            .filter_map(|(f, v)| Field::parse(f).map(|f| (f, Some(v.clone()))))
            .collect()
    };
    meta::set_extracted(conn, track_id, "musicbrainz", &to_fields(&found.fields))?;
    if !found.discogs_fields.is_empty() {
        meta::set_extracted(conn, track_id, "discogs", &to_fields(&found.discogs_fields))?;
    }
    for (namespace, value) in &found.external_ids {
        conn.execute(
            "INSERT OR IGNORE INTO track_external_id (track_id, namespace, value, source)
             VALUES (?1, ?2, ?3, 'acoustid')",
            params![track_id, namespace, value],
        )?;
    }
    Ok(())
}

/// Whether the best result can be applied without asking.
pub fn confident(results: &[Identified], duration_ms: i64) -> bool {
    let Some(best) = results.first() else {
        return false;
    };
    let length_ok = best
        .duration_ms
        .is_some_and(|d| (d - duration_ms).abs() <= LENGTH_TOLERANCE_MS);
    // A close second with a different title means the fingerprint is ambiguous.
    let title = |r: &Identified| {
        r.fields
            .iter()
            .find(|(f, _)| f == "title")
            .map(|(_, v)| crate::identity::normalize::title_key(v))
    };
    let rival = results
        .iter()
        .skip(1)
        .any(|r| r.score >= best.score - 0.05 && title(r) != title(best));
    best.score >= CONFIDENT_SCORE && length_ok && !rival
}

/// Store a lookup's results: apply a confident match, otherwise keep suggestions.
pub fn record(
    conn: &Connection,
    track_id: &str,
    duration_ms: i64,
    results: &[Identified],
    now: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM metadata_suggestion WHERE track_id = ?1",
        params![track_id],
    )?;
    if confident(results, duration_ms) {
        apply(&tx, track_id, &results[0])?;
        set_status(&tx, track_id, "identified", None, now)?;
        crate::sharing::after_identified(&tx, track_id, now)?;
    } else if results.is_empty() {
        set_status(&tx, track_id, "not_found", Some("No fingerprint match."), now)?;
    } else {
        for r in results.iter().take(5) {
            tx.execute(
                "INSERT INTO metadata_suggestion (id, track_id, score, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_id(), track_id, r.score, serde_json::to_string(r)?, now],
            )?;
        }
        set_status(
            &tx,
            track_id,
            "suggested",
            Some("Not certain; choose a match or leave as is."),
            now,
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub id: String,
    pub track_id: String,
    pub current: meta::TrackMeta,
    pub score: f64,
    pub found: Identified,
}

/// Tracks whose lookup found possible matches but none certain enough.
pub fn suggestions(conn: &Connection) -> Result<Vec<Suggestion>> {
    let rows: Vec<(String, String, f64, String)> = conn
        .prepare(
            "SELECT id, track_id, score, payload FROM metadata_suggestion ORDER BY track_id, score DESC",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter()
        .map(|(id, track_id, score, payload)| {
            Ok(Suggestion {
                current: meta::effective(conn, &track_id)?,
                found: serde_json::from_str(&payload)?,
                id,
                track_id,
                score,
            })
        })
        .collect()
}

/// Apply a suggestion the user chose.
pub fn accept(conn: &Connection, suggestion_id: &str, now: i64) -> Result<()> {
    let (track_id, payload): (String, String) = conn
        .query_row(
            "SELECT track_id, payload FROM metadata_suggestion WHERE id = ?1",
            params![suggestion_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound("suggestion".into()))?;
    let found: Identified = serde_json::from_str(&payload)?;
    let tx = conn.unchecked_transaction()?;
    apply(&tx, &track_id, &found)?;
    tx.execute(
        "DELETE FROM metadata_suggestion WHERE track_id = ?1",
        params![track_id],
    )?;
    set_status(&tx, &track_id, "identified", Some("Chosen by you."), now)?;
    crate::sharing::after_identified(&tx, &track_id, now)?;
    tx.commit()?;
    Ok(())
}

/// Keep the track's metadata as it is.
pub fn dismiss(conn: &Connection, track_id: &str, now: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM metadata_suggestion WHERE track_id = ?1",
        params![track_id],
    )?;
    set_status(conn, track_id, "not_found", Some("Kept as it was."), now)
}

#[derive(Serialize, Deserialize)]
struct Payload {
    track_id: String,
}

/// About two minutes of Chromaprint values, which is what AcoustID indexes.
const LOOKUP_ITEMS: usize = 1_000;

pub struct MetadataHandler {
    pub lookup: Arc<dyn MetadataLookup>,
}

impl Handler for MetadataHandler {
    fn kind(&self) -> &'static str {
        kinds::METADATA
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let Payload { track_id } = ctx.payload()?;
        let fingerprints = crate::analysis::store::fingerprints_of_track(ctx.conn, &track_id)?;
        let Some((_, values, duration_ms)) = fingerprints.into_iter().max_by_key(|f| f.2) else {
            set_status(
                ctx.conn,
                &track_id,
                "not_found",
                Some("Not analysed yet; it will be looked up after analysis."),
                ctx.now(),
            )?;
            return Ok(());
        };
        let m = meta::effective(ctx.conn, &track_id)?;
        let query = MetadataQuery {
            fingerprint: values.into_iter().take(LOOKUP_ITEMS).collect(),
            duration_ms,
            artist: m.artist,
            title: m.title,
        };
        match self.lookup.lookup(&query) {
            Ok(results) => {
                record(ctx.conn, &track_id, duration_ms, &results, ctx.now())?;
                Ok(())
            }
            Err(e) => {
                let status = if matches!(e, AdapterError::Auth(_)) {
                    "waiting"
                } else {
                    "failed"
                };
                set_status(ctx.conn, &track_id, status, Some(&e.to_string()), ctx.now())?;
                Err(ctx.adapter_error(e))
            }
        }
    }
}

#[cfg(test)]
#[path = "metadata_lookup_tests.rs"]
mod tests;
