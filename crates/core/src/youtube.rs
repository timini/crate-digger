//! YouTube reference links for tracks. Links are references only.
//!
//! Lookups replace earlier automatic results, but never touch a link the
//! user chose or rejected. Only a confident automatic match becomes the
//! preferred link; anything less stays an alternative, and the track stays
//! unresolved.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{AdapterError, VideoLookup, VideoMatch, VideoQuery};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::util::new_id;
use crate::{library, meta, Result};

/// Automatic matches at or above this confidence become the preferred link.
pub const PREFERRED_MIN: f64 = 0.7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupStatus {
    /// `queued`, `found`, `uncertain`, `none`, `waiting` or `failed`.
    pub status: String,
    pub detail: Option<String>,
    pub updated_at: i64,
}

fn set_status(conn: &Connection, track_id: &str, status: &str, detail: Option<&str>, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO youtube_lookup (track_id, status, detail, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET status = ?2, detail = ?3, updated_at = ?4",
        params![track_id, status, detail, now],
    )?;
    Ok(())
}

pub fn status(conn: &Connection, track_id: &str) -> Result<Option<LookupStatus>> {
    Ok(conn
        .query_row(
            "SELECT status, detail, updated_at FROM youtube_lookup WHERE track_id = ?1",
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

/// Queue a lookup. `refresh` starts a new one even if one ran before.
pub fn queue_lookup(conn: &Connection, track_id: &str, refresh: bool, now: i64) -> Result<()> {
    let key = if refresh {
        format!("youtube:{track_id}:{}", new_id())
    } else {
        format!("youtube:{track_id}")
    };
    let job = crate::jobs::enqueue(
        conn,
        &NewJob::new(kinds::YOUTUBE, key, serde_json::json!({ "track_id": track_id })).connector("youtube"),
        now,
    )?;
    if job.created {
        set_status(conn, track_id, "queued", None, now)?;
    }
    Ok(())
}

/// Recompute the status from the stored links.
fn refresh_status(conn: &Connection, track_id: &str, now: i64) -> Result<()> {
    let (preferred, alternatives): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(preferred), 0), COUNT(*) FROM youtube_match
         WHERE track_id = ?1 AND rejected = 0",
        params![track_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let status = match (preferred, alternatives) {
        (p, _) if p > 0 => "found",
        (_, a) if a > 0 => "uncertain",
        _ => "none",
    };
    set_status(conn, track_id, status, None, now)
}

/// Store a lookup's results. Earlier automatic results are replaced; links
/// the user chose or rejected are kept as they are.
pub fn save_results(conn: &Connection, track_id: &str, matches: &[VideoMatch], now: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM youtube_match WHERE track_id = ?1 AND user_corrected = 0",
        params![track_id],
    )?;
    let user_preferred: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM youtube_match WHERE track_id = ?1 AND preferred = 1)",
        params![track_id],
        |r| r.get(0),
    )?;
    let best = matches
        .iter()
        .filter(|m| m.confidence >= PREFERRED_MIN)
        .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
        .map(|m| m.video_id.clone());
    for m in matches {
        let preferred = !user_preferred && best.as_deref() == Some(m.video_id.as_str());
        tx.execute(
            "INSERT OR IGNORE INTO youtube_match
                 (id, track_id, video_id, url, title, channel, duration_ms, looked_up_at, confidence, preferred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                new_id(),
                track_id,
                m.video_id,
                m.url,
                m.title,
                m.channel,
                m.duration_ms,
                now,
                m.confidence.clamp(0.0, 1.0),
                preferred
            ],
        )?;
    }
    refresh_status(&tx, track_id, now)?;
    tx.commit()?;
    Ok(())
}

/// The user's own link, already confirmed to exist. It becomes the preferred
/// link and no lookup replaces it.
pub fn correct(conn: &Connection, track_id: &str, link: &VideoMatch, now: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE youtube_match SET preferred = 0 WHERE track_id = ?1",
        params![track_id],
    )?;
    tx.execute(
        "INSERT INTO youtube_match
             (id, track_id, video_id, url, title, channel, duration_ms, looked_up_at, confidence,
              preferred, user_corrected, rejected)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1.0, 1, 1, 0)
         ON CONFLICT(track_id, video_id) DO UPDATE SET
             url = ?4, title = COALESCE(?5, title), channel = COALESCE(?6, channel),
             looked_up_at = ?8, confidence = 1.0, preferred = 1, user_corrected = 1, rejected = 0",
        params![
            new_id(),
            track_id,
            link.video_id,
            link.url,
            link.title,
            link.channel,
            link.duration_ms,
            now
        ],
    )?;
    refresh_status(&tx, track_id, now)?;
    tx.commit()?;
    Ok(())
}

/// Choose one of the stored alternatives.
pub fn prefer(conn: &Connection, track_id: &str, video_id: &str, now: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute(
        "UPDATE youtube_match SET preferred = 1, user_corrected = 1, rejected = 0
         WHERE track_id = ?1 AND video_id = ?2",
        params![track_id, video_id],
    )?;
    if n == 0 {
        return Err(crate::Error::NotFound("YouTube link".into()));
    }
    tx.execute(
        "UPDATE youtube_match SET preferred = 0 WHERE track_id = ?1 AND video_id != ?2",
        params![track_id, video_id],
    )?;
    refresh_status(&tx, track_id, now)?;
    tx.commit()?;
    Ok(())
}

/// Mark a link wrong. It is hidden and later lookups cannot bring it back.
pub fn reject(conn: &Connection, track_id: &str, video_id: &str, now: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE youtube_match SET rejected = 1, user_corrected = 1, preferred = 0
         WHERE track_id = ?1 AND video_id = ?2",
        params![track_id, video_id],
    )?;
    if n == 0 {
        return Err(crate::Error::NotFound("YouTube link".into()));
    }
    refresh_status(conn, track_id, now)
}

#[derive(Serialize, Deserialize)]
struct Payload {
    track_id: String,
}

pub struct YoutubeHandler {
    pub lookup: Arc<dyn VideoLookup>,
}

impl Handler for YoutubeHandler {
    fn kind(&self) -> &'static str {
        kinds::YOUTUBE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let Payload { track_id } = ctx.payload()?;
        let m = meta::effective(ctx.conn, &track_id)?;
        let (Some(artist), Some(title)) = (m.artist, m.title) else {
            set_status(
                ctx.conn,
                &track_id,
                "none",
                Some("The track has no artist and title to search for."),
                ctx.now(),
            )?;
            return Ok(());
        };
        let query = VideoQuery {
            artist,
            title,
            mix: m.mix,
            duration_ms: library::playable_file(ctx.conn, &track_id)?.and_then(|f| f.duration_ms),
        };
        match self.lookup.lookup(&query) {
            Ok(matches) => {
                save_results(ctx.conn, &track_id, &matches, ctx.now())?;
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
#[path = "youtube_tests.rs"]
mod tests;
