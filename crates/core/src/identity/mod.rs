//! Canonical identity: which recording a file or candidate is, and how
//! tracks relate (same recording, versions of one work, unrelated).
//!
//! Decisions come from [`policy::decide`]; this module stores the evidence,
//! applies automatic verdicts and keeps a review queue for the rest.

pub mod normalize;
pub mod policy;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::library::{self, duplicates};
use crate::meta::{self, TrackMeta};
use crate::util::{new_id, now_ms};
use crate::{Error, Result};
use policy::{Decision, Evidence, Relation, Side, Verdict};

/// Below these, a fingerprint comparison is treated as "the audio differs".
/// Calibrated in tests/identity_calibration.rs.
pub const MIN_ALIGNMENT_SCORE: f64 = 0.6;
pub const MIN_ALIGNMENT_COVERAGE: f64 = 0.2;

/// Turn a fingerprint comparison (score and coverage from 0 to 1, and the
/// speed of B relative to A) into policy evidence.
pub fn fingerprint_evidence(score: f64, coverage: f64, speed: f64) -> Evidence {
    if score >= MIN_ALIGNMENT_SCORE && coverage >= MIN_ALIGNMENT_COVERAGE {
        Evidence::FingerprintMatch {
            score,
            coverage,
            speed,
        }
    } else {
        Evidence::FingerprintMismatch { score }
    }
}

/// Everything the policy needs to know about a stored track.
pub fn side_for_track(conn: &Connection, track_id: &str) -> Result<Side> {
    let m = meta::effective(conn, track_id)?;
    let duration_ms: Option<i64> = conn
        .query_row(
            "SELECT duration_ms FROM audio_file WHERE track_id = ?1 AND variant IS NULL
             ORDER BY is_primary DESC, created_at LIMIT 1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let external_ids = {
        let mut stmt = conn.prepare("SELECT namespace, value FROM track_external_id WHERE track_id = ?1")?;
        let rows = stmt
            .query_map(params![track_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<(String, String)>, _>>()?;
        rows
    };
    Ok(Side {
        artist: m.artist,
        title: m.title,
        mix: m.mix,
        duration_ms,
        external_ids,
    })
}

fn verdict_name(v: &Verdict) -> &'static str {
    match v {
        Verdict::SameRecording => "same_recording",
        Verdict::PitchedCopy { .. } => "pitched_copy",
        Verdict::DifferentVersion => "different_version",
        Verdict::Unrelated => "unrelated",
        Verdict::Unknown => "unknown",
        Verdict::NeedsReview { .. } => "needs_review",
    }
}

/// Keep a permanent record of a decision and the evidence behind it. Not
/// tied to the tracks by foreign key, so it outlives merges.
pub fn record_evidence(
    conn: &Connection,
    a: &str,
    b: &str,
    decision: &Decision,
    evidence: &[Evidence],
    source: &str,
) -> Result<()> {
    let detail = serde_json::json!({ "evidence": evidence, "reasons": decision.reasons });
    let kind = evidence
        .iter()
        .map(|e| {
            serde_json::to_value(e)
                .ok()
                .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        "INSERT INTO identity_evidence (id, track_a, track_b, kind, verdict, detail, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            new_id(),
            a,
            b,
            if kind.is_empty() {
                "metadata".to_string()
            } else {
                kind
            },
            verdict_name(&decision.verdict),
            detail.to_string(),
            source,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Put two tracks in the same work, creating or combining works as needed.
pub fn link_versions(conn: &Connection, a: &str, b: &str) -> Result<String> {
    let work_of = |t: &str| -> Result<Option<String>> {
        conn.query_row("SELECT work_id FROM track WHERE id = ?1", params![t], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("track {t}")))
    };
    let (wa, wb) = (work_of(a)?, work_of(b)?);
    let tx = conn.unchecked_transaction()?;
    let work = match (wa, wb) {
        (Some(x), Some(y)) if x == y => x,
        (Some(x), Some(y)) => {
            tx.execute("UPDATE track SET work_id = ?1 WHERE work_id = ?2", params![x, y])?;
            tx.execute("DELETE FROM work WHERE id = ?1", params![y])?;
            x
        }
        (Some(x), None) | (None, Some(x)) => x,
        (None, None) => {
            let id = new_id();
            tx.execute(
                "INSERT INTO work (id, created_at) VALUES (?1, ?2)",
                params![id, now_ms()],
            )?;
            id
        }
    };
    tx.execute(
        "UPDATE track SET work_id = ?1 WHERE id IN (?2, ?3)",
        params![work, a, b],
    )?;
    tx.commit()?;
    Ok(work)
}

/// Open a conflict for review, unless one is already open or the user
/// already decided this pair.
pub fn open_conflict(conn: &Connection, a: &str, b: &str, reason: &str, evidence: &[Evidence]) -> Result<()> {
    let (x, y) = if a < b { (a, b) } else { (b, a) };
    conn.execute(
        "INSERT OR IGNORE INTO identity_conflict (id, track_a, track_b, reason, evidence, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![new_id(), x, y, reason, serde_json::to_string(evidence)?, now_ms()],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Applied {
    Merged { kept: String },
    MergedAsPitchedCopy { kept: String, percent: f64 },
    LinkedAsVersions { work: String },
    SentToReview,
    NoChange,
}

/// Act on a decision about `keep` and `other`. Only automatic decisions
/// change anything; the rest are recorded and, if they conflict, queued.
/// For a pitched copy, `other` is the pitched one.
pub fn apply(
    conn: &Connection,
    keep: &str,
    other: &str,
    decision: &Decision,
    evidence: &[Evidence],
    source: &str,
) -> Result<Applied> {
    record_evidence(conn, keep, other, decision, evidence, source)?;
    let summary = decision.reasons.join(" ");
    match &decision.verdict {
        Verdict::SameRecording if decision.automatic => {
            duplicates::merge(conn, keep, other, &format!("{source}: {summary}"))?;
            Ok(Applied::Merged { kept: keep.into() })
        }
        Verdict::PitchedCopy { percent } if decision.automatic => {
            let moved: Vec<String> = {
                let mut stmt = conn.prepare("SELECT id FROM audio_file WHERE track_id = ?1")?;
                let rows = stmt
                    .query_map(params![other], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<String>, _>>()?;
                rows
            };
            duplicates::merge(conn, keep, other, &format!("{source}: {summary}"))?;
            for f in moved {
                conn.execute(
                    "UPDATE audio_file SET variant = ?2, is_primary = 0 WHERE id = ?1",
                    params![f, format!("pitched:{percent:+.1}")],
                )?;
            }
            // The unpitched copy is the one to play.
            conn.execute(
                "UPDATE audio_file SET is_primary = (id = (
                     SELECT id FROM audio_file WHERE track_id = ?1
                     ORDER BY variant IS NOT NULL, created_at LIMIT 1))
                 WHERE track_id = ?1",
                params![keep],
            )?;
            Ok(Applied::MergedAsPitchedCopy {
                kept: keep.into(),
                percent: *percent,
            })
        }
        Verdict::DifferentVersion if decision.automatic => Ok(Applied::LinkedAsVersions {
            work: link_versions(conn, keep, other)?,
        }),
        Verdict::NeedsReview { reason } => {
            open_conflict(conn, keep, other, reason, evidence)?;
            Ok(Applied::SentToReview)
        }
        _ => Ok(Applied::NoChange),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConflictSide {
    pub track_id: String,
    pub meta: TrackMeta,
    pub path: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub id: String,
    pub a: ConflictSide,
    pub b: ConflictSide,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub created_at: i64,
}

fn conflict_side(conn: &Connection, track_id: &str) -> Result<ConflictSide> {
    let file = library::playable_file(conn, track_id)?;
    Ok(ConflictSide {
        track_id: track_id.to_string(),
        meta: meta::effective(conn, track_id)?,
        path: file.as_ref().map(|f| f.path.clone()),
        duration_ms: file.and_then(|f| f.duration_ms),
    })
}

pub fn open_conflicts(conn: &Connection) -> Result<Vec<Conflict>> {
    let rows: Vec<(String, String, String, String, String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT id, track_a, track_b, reason, evidence, created_at FROM identity_conflict
             WHERE state = 'open' ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    rows.into_iter()
        .map(|(id, a, b, reason, evidence, created_at)| {
            Ok(Conflict {
                id,
                a: conflict_side(conn, &a)?,
                b: conflict_side(conn, &b)?,
                reason,
                evidence: serde_json::from_str(&evidence).unwrap_or(serde_json::Value::Null),
                created_at,
            })
        })
        .collect()
}

/// Apply the user's answer to a conflict. For "same recording", track A is
/// kept and B merged into it.
pub fn resolve_conflict(conn: &Connection, conflict_id: &str, relation: Relation) -> Result<Applied> {
    let (a, b): (String, String) = conn
        .query_row(
            "SELECT track_a, track_b FROM identity_conflict WHERE id = ?1 AND state = 'open'",
            params![conflict_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("open conflict {conflict_id}")))?;
    let resolution = match relation {
        Relation::SameRecording => "same_recording",
        Relation::DifferentVersion => "different_version",
        Relation::Unrelated => "unrelated",
    };
    // Mark it resolved before a merge deletes track B (and the row with it).
    conn.execute(
        "UPDATE identity_conflict SET state = 'resolved', resolution = ?2, resolved_at = ?3 WHERE id = ?1",
        params![conflict_id, resolution, now_ms()],
    )?;
    let evidence = [Evidence::UserDecision { relation }];
    let decision = policy::decide(&Side::default(), &Side::default(), &evidence, &Default::default());
    let applied = apply(conn, &a, &b, &decision, &evidence, "user")?;
    if relation != Relation::SameRecording {
        duplicates::dismiss(conn, &a, &b)?;
    }
    Ok(applied)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Version {
    pub track_id: String,
    pub meta: TrackMeta,
    pub has_audio: bool,
}

/// Other tracks of the same work.
pub fn versions(conn: &Connection, track_id: &str) -> Result<Vec<Version>> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT t.id FROM track t JOIN track me ON me.work_id = t.work_id
             WHERE me.id = ?1 AND t.id != ?1 ORDER BY t.created_at",
        )?;
        let rows = stmt
            .query_map(params![track_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        rows
    };
    ids.into_iter()
        .map(|id| {
            Ok(Version {
                meta: meta::effective(conn, &id)?,
                has_audio: library::playable_file(conn, &id)?.is_some(),
                track_id: id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
