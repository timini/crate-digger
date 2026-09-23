//! The review queue and rating semantics.
//!
//! - A rating (thumbs down, one to three stars) is an explicit preference.
//! - A skip is not a preference; it hides the track for the current session.
//! - Undo reverses the session's most recent rating or skip, restoring the
//!   previous effective preference.
//! - Keeping audio and playlist membership are separate decisions; a rating
//!   never retains audio by itself.
//!
//! Only candidates with playable local audio are ready. A YouTube link on
//! its own never makes a track ready.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::domain::{RatingKind, Stage};
use crate::library::{self, FileRecord};
use crate::meta::{self, TrackMeta};
use crate::util::new_id;
use crate::{pipeline, playlists, Error, Result};

/// Candidates that can be reviewed right now in `session`.
const READY_WHERE: &str = "c.stage = 'ready' AND c.status = 'active'
    AND EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = c.track_id AND f.availability = 'available')
    AND NOT EXISTS (
        SELECT 1 FROM rating_event s
        WHERE s.track_id = c.track_id AND s.kind = 'skip' AND s.session_id = ?1
          AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = s.id))";

/// Move a candidate to Ready. Refused unless the track has playable local
/// audio.
pub fn mark_ready(conn: &Connection, candidate_id: &str, now: i64) -> Result<()> {
    let track_id: String = conn
        .query_row(
            "SELECT track_id FROM candidate WHERE id = ?1",
            params![candidate_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("candidate {candidate_id}")))?;
    if library::playable_file(conn, &track_id)?.is_none() {
        return Err(Error::Invalid(
            "a track needs playable local audio before it can be reviewed".into(),
        ));
    }
    pipeline::transition(conn, candidate_id, Stage::Ready, now)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueStats {
    /// Playable and waiting for review in this session.
    pub ready: i64,
    /// Found but without playable audio yet.
    pub in_progress: i64,
    /// Skipped earlier in this session.
    pub skipped: i64,
    pub needs_review: i64,
    pub failed: i64,
    pub reviewed: i64,
}

pub fn stats(conn: &Connection, session: &str) -> Result<QueueStats> {
    let ready: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM candidate c WHERE {READY_WHERE}"),
        params![session],
        |r| r.get(0),
    )?;
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    let ready_any: i64 = count(
        "SELECT COUNT(*) FROM candidate c WHERE c.stage = 'ready' AND c.status = 'active'
           AND EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = c.track_id AND f.availability = 'available')",
    )?;
    Ok(QueueStats {
        ready,
        in_progress: count(
            "SELECT COUNT(*) FROM candidate WHERE status = 'active'
               AND stage IN ('candidate', 'identified', 'acquisition_queued', 'downloading', 'validating', 'analysing')",
        )?,
        skipped: ready_any - ready,
        needs_review: count("SELECT COUNT(*) FROM candidate WHERE status = 'blocked'")?,
        failed: count("SELECT COUNT(*) FROM candidate WHERE status = 'failed'")?,
        reviewed: count("SELECT COUNT(*) FROM candidate WHERE stage = 'reviewed'")?,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub source_kind: String,
    pub source_url: Option<String>,
    pub supplied_text_id: Option<String>,
    pub retrieved_at: i64,
    pub excerpt: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct YoutubeLink {
    pub video_id: String,
    pub url: String,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub duration_ms: Option<i64>,
    pub confidence: f64,
    pub preferred: bool,
    pub user_corrected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewCard {
    pub candidate_id: String,
    pub track_id: String,
    pub meta: TrackMeta,
    pub file: FileRecord,
    pub reasons: Vec<String>,
    pub evidence: Vec<Evidence>,
    pub youtube: Vec<YoutubeLink>,
    pub youtube_status: Option<crate::youtube::LookupStatus>,
    pub confidence: Option<f64>,
    pub verified: bool,
    pub kept: bool,
    pub playlists: Vec<(String, String)>,
}

fn card(conn: &Connection, candidate_id: &str) -> Result<Option<ReviewCard>> {
    let Some((track_id, confidence, verified)) = conn
        .query_row(
            "SELECT track_id, confidence, verified FROM candidate WHERE id = ?1",
            params![candidate_id],
            |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let Some(file) = library::playable_file(conn, &track_id)? else {
        return Ok(None);
    };
    let reasons = {
        let mut stmt =
            conn.prepare("SELECT reason FROM explanation WHERE candidate_id = ?1 ORDER BY weight DESC")?;
        let mut rows = stmt
            .query_map(params![candidate_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        let note: Option<String> = conn.query_row(
            "SELECT rank_note FROM candidate WHERE id = ?1",
            params![candidate_id],
            |r| r.get(0),
        )?;
        rows.extend(note);
        rows
    };
    let evidence = {
        let mut stmt = conn.prepare(
            "SELECT source_kind, source_url, supplied_text_id, retrieved_at, excerpt, confidence
             FROM evidence WHERE candidate_id = ?1 ORDER BY confidence DESC",
        )?;
        let rows = stmt
            .query_map(params![candidate_id], |r| {
                Ok(Evidence {
                    source_kind: r.get(0)?,
                    source_url: r.get(1)?,
                    supplied_text_id: r.get(2)?,
                    retrieved_at: r.get(3)?,
                    excerpt: r.get(4)?,
                    confidence: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let youtube = {
        let mut stmt = conn.prepare(
            "SELECT video_id, url, title, channel, duration_ms, confidence, preferred, user_corrected
             FROM youtube_match WHERE track_id = ?1 AND rejected = 0
             ORDER BY preferred DESC, user_corrected DESC, confidence DESC",
        )?;
        let rows = stmt
            .query_map(params![track_id], |r| {
                Ok(YoutubeLink {
                    video_id: r.get(0)?,
                    url: r.get(1)?,
                    title: r.get(2)?,
                    channel: r.get(3)?,
                    duration_ms: r.get(4)?,
                    confidence: r.get(5)?,
                    preferred: r.get(6)?,
                    user_corrected: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    Ok(Some(ReviewCard {
        candidate_id: candidate_id.to_string(),
        meta: meta::effective(conn, &track_id)?,
        file,
        reasons,
        evidence,
        youtube_status: crate::youtube::status(conn, &track_id)?,
        youtube,
        confidence,
        verified,
        kept: is_kept(conn, &track_id)?,
        playlists: playlists::containing(conn, &track_id)?,
        track_id,
    }))
}

/// The next cards to review, best first.
pub fn next(conn: &Connection, session: &str, limit: i64) -> Result<Vec<ReviewCard>> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT c.id FROM candidate c WHERE {READY_WHERE}
             ORDER BY c.queue_rank IS NULL, c.queue_rank, c.score IS NULL, c.score DESC, c.created_at LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(params![session, limit], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        rows
    };
    let mut cards = Vec::new();
    for id in ids {
        if let Some(c) = card(conn, &id)? {
            cards.push(c);
        }
    }
    Ok(cards)
}

fn candidate_of(conn: &Connection, track_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM candidate WHERE track_id = ?1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()?)
}

fn insert_event(
    conn: &Connection,
    track_id: &str,
    kind: RatingKind,
    session: &str,
    undoes: Option<&str>,
    now: i64,
) -> Result<String> {
    let id = new_id();
    conn.execute(
        "INSERT INTO rating_event (id, track_id, kind, session_id, undoes_event_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, track_id, kind, session, undoes, now],
    )?;
    Ok(id)
}

pub fn effective_rating(conn: &Connection, track_id: &str) -> Result<Option<RatingKind>> {
    Ok(conn
        .query_row(
            "SELECT kind FROM effective_rating WHERE track_id = ?1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Record a rating. It is on disk when this returns; reranking happens
/// separately.
pub fn rate(conn: &Connection, track_id: &str, kind: RatingKind, session: &str, now: i64) -> Result<String> {
    if matches!(kind, RatingKind::Skip | RatingKind::Undo) {
        return Err(Error::Invalid(format!("{kind} is not a rating")));
    }
    let tx = conn.unchecked_transaction()?;
    let id = insert_event(&tx, track_id, kind, session, None, now)?;
    if let Some(c) = candidate_of(&tx, track_id)? {
        if pipeline::state(&tx, &c)?.stage == Stage::Ready {
            pipeline::transition(&tx, &c, Stage::Reviewed, now)?;
        }
    }
    tx.commit()?;
    Ok(id)
}

/// Defer a track for this session. Not a preference.
pub fn skip(conn: &Connection, track_id: &str, session: &str, now: i64) -> Result<String> {
    insert_event(conn, track_id, RatingKind::Skip, session, None, now)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Undone {
    pub track_id: String,
    pub kind: RatingKind,
    /// The preference now in effect for the track.
    pub effective: Option<RatingKind>,
}

/// Reverse the most recent rating or skip in this session that has not
/// already been undone. Repeating walks further back.
pub fn undo(conn: &Connection, session: &str, now: i64) -> Result<Option<Undone>> {
    let tx = conn.unchecked_transaction()?;
    let last: Option<(String, String, RatingKind)> = tx
        .query_row(
            "SELECT e.id, e.track_id, e.kind FROM rating_event e
             WHERE e.session_id = ?1 AND e.kind != 'undo'
               AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = e.id)
             ORDER BY e.rowid DESC LIMIT 1",
            params![session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((event_id, track_id, kind)) = last else {
        return Ok(None);
    };
    insert_event(&tx, &track_id, RatingKind::Undo, session, Some(&event_id), now)?;
    let effective = effective_rating(&tx, &track_id)?;
    if effective.is_none() {
        if let Some(c) = candidate_of(&tx, &track_id)? {
            if pipeline::state(&tx, &c)?.stage == Stage::Reviewed {
                pipeline::transition(&tx, &c, Stage::Ready, now)?;
            }
        }
    }
    tx.commit()?;
    Ok(Some(Undone {
        track_id,
        kind,
        effective,
    }))
}

pub fn is_kept(conn: &Connection, track_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM keep_decision WHERE track_id = ?1)",
        params![track_id],
        |r| r.get(0),
    )?)
}

/// Record the decision to keep a track's audio. Moving it into the archive
/// is a separate step.
pub fn keep(conn: &Connection, track_id: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO keep_decision (track_id, decided_at) VALUES (?1, ?2)",
        params![track_id, now],
    )?;
    Ok(())
}

pub fn unkeep(conn: &Connection, track_id: &str) -> Result<()> {
    conn.execute("DELETE FROM keep_decision WHERE track_id = ?1", params![track_id])?;
    Ok(())
}

/// Recompute scores and the review order after preferences change.
pub fn rerank(
    conn: &Connection,
    v: &crate::analysis::FeatureVersion,
) -> Result<crate::ranking::RerankSummary> {
    crate::ranking::rerank(conn, v, &crate::ranking::DEFAULT)
}

#[cfg(test)]
mod tests;
