//! Playlists as discovery workspaces (#6).
//!
//! Each playlist has a brief, its own seeds and its own queue of
//! suggestions. Feedback on whether a track fits a playlist is kept per
//! playlist and never changes the track's personal rating, so a track can
//! be wrong for a warm-up set and right for a peak-time one. Only an
//! explicit "Add to playlist" changes membership, and it goes at the end,
//! so the user's order is never rearranged.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::Seed;
use crate::analysis::{FeatureVersion, UnitEmbedding};
use crate::identity::normalize::fold;
use crate::jobs::{kinds, NewJob};
use crate::util::new_id;
use crate::{playlists, Error, Result};

pub const MAX_BRIEF: usize = 2_000;
pub const MAX_SEEDS: usize = 100;

fn exists(conn: &Connection, playlist: &str) -> Result<()> {
    conn.query_row("SELECT 1 FROM playlist WHERE id = ?1", params![playlist], |_| {
        Ok(())
    })
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("playlist {playlist}")))
}

pub fn set_brief(conn: &Connection, playlist: &str, brief: &str) -> Result<()> {
    let brief = brief.trim();
    if brief.chars().count() > MAX_BRIEF {
        return Err(Error::Invalid(format!(
            "Keep the brief under {MAX_BRIEF} characters."
        )));
    }
    exists(conn, playlist)?;
    conn.execute(
        "UPDATE playlist SET brief = ?2, updated_at = ?3 WHERE id = ?1",
        params![playlist, brief, crate::util::now_ms()],
    )?;
    Ok(())
}

/// Turn background discovery for this playlist on or off.
pub fn set_discovery(conn: &Connection, playlist: &str, on: bool) -> Result<()> {
    exists(conn, playlist)?;
    conn.execute(
        "UPDATE playlist SET discovery = ?2 WHERE id = ?1",
        params![playlist, on],
    )?;
    Ok(())
}

/// A playlist's brief, or None if the playlist does not exist.
pub fn brief(conn: &Connection, playlist: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT brief FROM playlist WHERE id = ?1",
            params![playlist],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn seeds(conn: &Connection, playlist: &str) -> Result<Vec<Seed>> {
    let mut stmt = conn
        .prepare("SELECT kind, value FROM playlist_seed WHERE playlist_id = ?1 ORDER BY created_at, rowid")?;
    let rows = stmt.query_map(params![playlist], |r| {
        Ok(Seed {
            kind: r.get(0)?,
            value: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Replace a playlist's seeds.
pub fn save_seeds(conn: &Connection, playlist: &str, seeds: &[Seed], now: i64) -> Result<()> {
    if seeds.len() > MAX_SEEDS
        || seeds
            .iter()
            .any(|s| s.value.trim().is_empty() || s.value.len() > 500)
    {
        return Err(Error::Invalid(format!(
            "Enter up to {MAX_SEEDS} seeds, each between 1 and 500 characters."
        )));
    }
    exists(conn, playlist)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM playlist_seed WHERE playlist_id = ?1",
        params![playlist],
    )?;
    for s in seeds {
        tx.execute(
            "INSERT OR IGNORE INTO playlist_seed (playlist_id, kind, value, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![playlist, s.kind, s.value.trim(), now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Seeds for a discovery run for this playlist: its own seeds, then the
/// artists and labels of the tracks accepted into it. Global seeds and
/// ratings are not used, so each playlist looks in its own direction.
pub fn seeds_for_run(conn: &Connection, playlist: &str) -> Result<Vec<Seed>> {
    use crate::domain::SeedKind;
    let mut out = seeds(conn, playlist)?;
    let mut stmt = conn.prepare(
        "SELECT m.artist, m.label FROM playlist_entry e JOIN track_meta m ON m.track_id = e.track_id
         WHERE e.playlist_id = ?1 ORDER BY e.added_at DESC, e.position DESC LIMIT 30",
    )?;
    let rows = stmt
        .query_map(params![playlist], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (artist, label) in rows {
        for (kind, value) in [(SeedKind::Artist, artist), (SeedKind::Label, label)] {
            let Some(value) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
                continue;
            };
            if !out
                .iter()
                .any(|s| s.kind == kind && s.value.eq_ignore_ascii_case(&value))
            {
                out.push(Seed { kind, value });
            }
        }
    }
    Ok(out)
}

/// Record that a candidate was suggested for a playlist.
pub fn add_context(conn: &Connection, candidate_id: &str, playlist: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO candidate_context (candidate_id, playlist_id, created_at) VALUES (?1, ?2, ?3)",
        params![candidate_id, playlist, now],
    )?;
    Ok(())
}

/// The playlists a candidate was suggested for: (id, name).
pub fn contexts(conn: &Connection, candidate_id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.name FROM candidate_context c JOIN playlist p ON p.id = c.playlist_id
         WHERE c.candidate_id = ?1 ORDER BY p.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![candidate_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Fits,
    NotForThis,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Fits => "fits",
            Verdict::NotForThis => "not_for_this",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "fits" => Some(Verdict::Fits),
            "not_for_this" => Some(Verdict::NotForThis),
            _ => None,
        }
    }
}

/// The current verdict on a track for a playlist.
pub fn verdict(conn: &Connection, playlist: &str, track: &str) -> Result<Option<Verdict>> {
    Ok(conn
        .query_row(
            "SELECT verdict FROM playlist_feedback WHERE playlist_id = ?1 AND track_id = ?2 AND undone_at IS NULL
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![playlist, track],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| Verdict::parse(&v)))
}

/// Say whether a track fits a playlist. "Fits" also adds it to the end of
/// the playlist unless it is already there. Ratings are not touched.
pub fn give_feedback(conn: &Connection, playlist: &str, track: &str, v: Verdict, now: i64) -> Result<String> {
    exists(conn, playlist)?;
    let tx = conn.unchecked_transaction()?;
    let member = playlists::track_ids(&tx, playlist)?.iter().any(|t| t == track);
    let add = v == Verdict::Fits && !member;
    if add {
        playlists::add_tracks(&tx, playlist, &[track.to_string()], None)?;
    }
    let id = new_id();
    tx.execute(
        "INSERT INTO playlist_feedback (id, playlist_id, track_id, verdict, added_to_playlist, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, playlist, track, v.as_str(), add, now],
    )?;
    tx.commit()?;
    Ok(id)
}

/// Undo the latest feedback in a playlist. If it added the track, the
/// entry it added (the last one for that track) is removed. Returns the track.
pub fn undo_feedback(conn: &Connection, playlist: &str, now: i64) -> Result<Option<String>> {
    let tx = conn.unchecked_transaction()?;
    let Some((id, track, added)): Option<(String, String, bool)> = tx
        .query_row(
            "SELECT id, track_id, added_to_playlist FROM playlist_feedback
             WHERE playlist_id = ?1 AND undone_at IS NULL ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![playlist],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE playlist_feedback SET undone_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    if added {
        if let Some(pos) = playlists::track_ids(&tx, playlist)?
            .iter()
            .rposition(|t| *t == track)
        {
            playlists::remove_entry(&tx, playlist, pos)?;
        }
    }
    tx.commit()?;
    Ok(Some(track))
}

/// A suggestion in a playlist's queue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub candidate_id: String,
    pub track_id: String,
    pub score: f32,
    /// Why it is suggested for this playlist, most important first.
    pub reasons: Vec<String>,
}

/// Weights for playlist fit. Sound similarity to the accepted tracks
/// counts most; personal ranking and playlist seeds break ties.
const W_FIT: f32 = 0.6;
const W_PERSONAL: f32 = 0.25;
const W_SEED: f32 = 0.15;
const W_REJECT: f32 = 0.6;

fn mean_top(mut sims: Vec<f32>, k: usize) -> Option<f32> {
    if sims.is_empty() {
        return None;
    }
    sims.sort_by(|a, b| b.total_cmp(a));
    let n = sims.len().min(k);
    Some(sims[..n].iter().sum::<f32>() / n as f32)
}

/// Suggestions for a playlist that have audio to play, best first. A
/// track rated elsewhere still needs a decision here, so reviewed
/// candidates stay while their audio does. A suggestion leaves the queue
/// once the user gives feedback on it for this playlist or it is in the
/// playlist. A track the user gave thumbs down to is never suggested.
pub fn queue(conn: &Connection, playlist: &str, v: &FeatureVersion, limit: usize) -> Result<Vec<Suggestion>> {
    // (candidate, track, personal score, artist, label)
    type Row = (String, String, f64, Option<String>, Option<String>);
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.track_id, COALESCE(c.score, 0), m.artist, m.label
             FROM candidate_context x
             JOIN candidate c ON c.id = x.candidate_id
             LEFT JOIN track_meta m ON m.track_id = c.track_id
             WHERE x.playlist_id = ?1 AND c.stage IN ('ready', 'reviewed') AND c.status = 'active'
               AND EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = c.track_id AND f.availability = 'available')
               AND NOT EXISTS (SELECT 1 FROM playlist_entry e WHERE e.playlist_id = ?1 AND e.track_id = c.track_id)
               AND NOT EXISTS (SELECT 1 FROM playlist_feedback f WHERE f.playlist_id = ?1
                               AND f.track_id = c.track_id AND f.undone_at IS NULL)
               AND NOT EXISTS (SELECT 1 FROM effective_rating r WHERE r.track_id = c.track_id
                               AND r.kind = 'thumbs_down')",
        )?;
        let rows = stmt.query_map(params![playlist], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let vectors = crate::ranking::embeddings(conn, v)?;
    let members: Vec<&UnitEmbedding> = playlists::track_ids(conn, playlist)?
        .iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter_map(|t| vectors.get(t))
        .collect();
    let rejected: Vec<&UnitEmbedding> = {
        let mut stmt = conn.prepare(
            "SELECT track_id FROM playlist_feedback f WHERE playlist_id = ?1 AND undone_at IS NULL
               AND verdict = 'not_for_this'
               AND rowid = (SELECT MAX(rowid) FROM playlist_feedback g WHERE g.playlist_id = f.playlist_id
                            AND g.track_id = f.track_id AND g.undone_at IS NULL)",
        )?;
        let ids = stmt
            .query_map(params![playlist], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.iter().filter_map(|t| vectors.get(t)).collect()
    };
    let seeds: HashSet<String> = seeds(conn, playlist)?.iter().map(|s| fold(&s.value)).collect();
    let mut out: Vec<Suggestion> = rows
        .into_iter()
        .map(|(candidate_id, track_id, personal, artist, label)| {
            let mut reasons = vec![];
            let mut score = W_PERSONAL * personal as f32;
            let e = vectors.get(&track_id);
            if let Some(e) = e {
                let sims = |set: &[&UnitEmbedding]| -> Vec<f32> {
                    set.iter().filter_map(|m| m.cosine(e).ok()).collect()
                };
                let fit = mean_top(sims(&members), 3);
                if let Some(fit) = fit {
                    score += W_FIT * fit.max(0.0);
                    if fit >= 0.8 {
                        reasons.push("Sounds like tracks already in this playlist".to_string());
                    }
                }
                let reject = sims(&rejected).into_iter().fold(f32::MIN, f32::max);
                if reject > fit.unwrap_or(0.0).max(0.5) {
                    score -= W_REJECT * (reject - fit.unwrap_or(0.0).max(0.5));
                    reasons.push("Close to a track you said does not fit here".to_string());
                }
            }
            let seed_hit = artist.as_deref().is_some_and(|a| seeds.contains(&fold(a)))
                || label
                    .as_deref()
                    .is_some_and(|l| !l.is_empty() && seeds.contains(&fold(l)));
            if seed_hit {
                score += W_SEED;
                reasons.insert(0, "Artist or label is a seed for this playlist".to_string());
            }
            Suggestion {
                candidate_id,
                track_id,
                score,
                reasons,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.candidate_id.cmp(&b.candidate_id))
    });
    out.truncate(limit);
    Ok(out)
}

/// Ready suggestions waiting in each playlist's queue, by playlist id.
pub fn ready_counts(conn: &Connection, v: &FeatureVersion) -> Result<HashMap<String, usize>> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM playlist")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = HashMap::new();
    for id in ids {
        out.insert(id.clone(), queue(conn, &id, v, usize::MAX)?.len());
    }
    Ok(out)
}

fn job_key_prefix(playlist: &str) -> String {
    format!("discover:{playlist}:")
}

/// Queue a discovery run for a playlist, from its brief and seeds.
pub fn request_discovery(
    conn: &Connection,
    connector: &str,
    playlist: &str,
    limit: usize,
    now: i64,
) -> Result<String> {
    exists(conn, playlist)?;
    Ok(crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::DISCOVER,
            format!("{}{}", job_key_prefix(playlist), new_id()),
            serde_json::json!({ "limit": limit, "input": {"kind": "seeds"}, "playlist": playlist }),
        )
        .connector(connector),
        now,
    )?
    .id)
}

/// Playlists that want more suggestions now: discovery is on, fewer than
/// `below` suggestions are ready, no run is waiting, and none started
/// within `interval_ms`.
pub fn refresh_due(
    conn: &Connection,
    v: &FeatureVersion,
    below: usize,
    interval_ms: i64,
    now: i64,
) -> Result<Vec<String>> {
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM playlist WHERE discovery = 1 ORDER BY created_at")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut due = vec![];
    for id in ids {
        let pending: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM job WHERE kind = ?1
                              AND substr(idempotency_key, 1, length(?2)) = ?2
                              AND state IN ('queued', 'running', 'blocked', 'paused'))",
            params![kinds::DISCOVER, job_key_prefix(&id)],
            |r| r.get(0),
        )?;
        if pending {
            continue;
        }
        let last: Option<i64> = conn.query_row(
            "SELECT MAX(started_at) FROM source_run WHERE playlist_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if last.is_some_and(|t| now - t < interval_ms) {
            continue;
        }
        if queue(conn, &id, v, below)?.len() < below {
            due.push(id);
        }
    }
    Ok(due)
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
