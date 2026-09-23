//! Matching a newly analysed file against the library.
//!
//! Likely matches are found by shared fingerprint values and by title, then
//! each is compared by audio (with speed compensation when the lengths
//! suggest a pitched copy) and judged by the policy. Library tracks are
//! merged, linked or sent to review; a discovery candidate that turns out
//! to be something the user already owns is held back instead of merged.

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;

use super::fingerprint::compare_with_speed;
use super::policy::{decide, Decision, Evidence, Thresholds, Verdict};
use super::{apply, fingerprint_evidence, link_versions, record_evidence, side_for_track, Applied};
use crate::analysis::handler::Analyzer;
use crate::analysis::protocol::FingerprintOut;
use crate::analysis::store;
use crate::domain::{CandidateStatus, Stage};
use crate::util::{new_id, now_ms};
use crate::{library, meta, pipeline, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatchOutcome {
    pub other: String,
    pub decision: Decision,
    pub applied: Applied,
}

fn to_out(fp: store::StoredFingerprint) -> FingerprintOut {
    FingerprintOut {
        algorithm: fp.0,
        data: fp.1,
        duration_ms: fp.2.max(0) as u64,
        speed: 1.0,
    }
}

/// Tracks worth comparing with `track_id`: those sharing fingerprint values
/// with `fp`, and those with the same folded title.
fn likely_matches(conn: &Connection, track_id: &str, fp: Option<&FingerprintOut>) -> Result<Vec<String>> {
    let mut out = BTreeSet::new();
    if let Some(fp) = fp {
        let keys: Vec<i64> = fp
            .data
            .iter()
            .step_by(3)
            .filter(|v| **v != 0)
            .map(|v| *v as i64)
            .collect();
        for chunk in keys.chunks(900) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT f.track_id, COUNT(*) AS n FROM fingerprint_key k
                 JOIN fingerprint f ON f.id = k.fingerprint_id
                 WHERE k.key IN ({placeholders}) AND f.track_id != ?
                 GROUP BY f.track_id HAVING n >= 3 ORDER BY n DESC LIMIT 10"
            );
            let mut args: Vec<rusqlite::types::Value> = chunk.iter().map(|k| (*k).into()).collect();
            args.push(track_id.to_string().into());
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out.extend(rows);
        }
    }
    let title_key: Option<String> = conn
        .query_row(
            "SELECT title_key FROM track_meta WHERE track_id = ?1",
            params![track_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(k) = title_key.filter(|k| !k.is_empty()) {
        let mut stmt =
            conn.prepare("SELECT track_id FROM track_meta WHERE title_key = ?1 AND track_id != ?2 LIMIT 20")?;
        let rows = stmt
            .query_map(params![k, track_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out.extend(rows);
    }
    Ok(out.into_iter().collect())
}

/// The user already decided about this pair (or dismissed it as a
/// duplicate), so it is not reconsidered automatically.
fn already_decided(conn: &Connection, a: &str, b: &str) -> Result<bool> {
    let (x, y) = if a < b { (a, b) } else { (b, a) };
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM identity_conflict WHERE track_a = ?1 AND track_b = ?2)
             OR EXISTS (SELECT 1 FROM duplicate_dismissal WHERE track_a = ?1 AND track_b = ?2)",
        params![x, y],
        |r| r.get(0),
    )?)
}

/// Whether the user owns audio for this track (imported or archived).
fn owned(conn: &Connection, track_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM audio_file WHERE track_id = ?1 AND origin IN ('imported', 'archived'))",
        params![track_id],
        |r| r.get(0),
    )?)
}

fn open_candidate(conn: &Connection, track_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM candidate WHERE track_id = ?1 AND stage != 'reviewed'",
            params![track_id],
            |r| r.get(0),
        )
        .optional()?)
}

fn label(conn: &Connection, track_id: &str) -> Result<String> {
    let m = meta::effective(conn, track_id)?;
    let title = m.title.unwrap_or_else(|| "Untitled".into());
    let title = match m.mix {
        Some(mix) => format!("{title} ({mix})"),
        None => title,
    };
    Ok(match m.artist {
        Some(a) => format!("{a} - {title}"),
        None => title,
    })
}

fn explain(conn: &Connection, candidate_id: &str, reason: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO explanation (id, candidate_id, reason, weight) VALUES (?1, ?2, ?3, 1)",
        params![new_id(), candidate_id, reason],
    )?;
    Ok(())
}

/// Evidence about `other` (A) and `me` (B), fingerprinting my file at
/// other speeds if the lengths suggest a pitched copy.
fn evidence_for(
    conn: &Connection,
    analyzer: &dyn Analyzer,
    my_fp: Option<&FingerprintOut>,
    my_path: &Path,
    other: &str,
) -> Result<Vec<Evidence>> {
    let Some(mine) = my_fp else { return Ok(Vec::new()) };
    let theirs = store::fingerprints_of_track(conn, other)?;
    let mut best: Option<(f64, f64, f64)> = None;
    for fp in theirs {
        let a = to_out(fp);
        let found = compare_with_speed(
            &a,
            |speed| {
                if (speed - 1.0).abs() < 1e-9 {
                    Some(mine.clone())
                } else {
                    analyzer.fingerprint(my_path, speed).ok()
                }
            },
            mine.duration_ms,
        );
        if let Some((c, speed)) = found {
            if best.map(|b| c.score * c.coverage > b.0 * b.1).unwrap_or(true) {
                best = Some((c.score, c.coverage, speed));
            }
        }
    }
    Ok(best
        .map(|(s, c, speed)| vec![fingerprint_evidence(s, c, speed)])
        .unwrap_or_default())
}

/// Match `track_id` (whose file `file_id` was just analysed) against the
/// library and act on the verdicts. Stops early if the track is merged
/// into another.
pub fn match_track(
    conn: &Connection,
    analyzer: &dyn Analyzer,
    track_id: &str,
    file_id: &str,
    t: &Thresholds,
) -> Result<Vec<MatchOutcome>> {
    let mut outcomes = Vec::new();
    let my_fp = store::fingerprint_of_file(conn, file_id)?.map(to_out);
    let my_path = library::file(conn, file_id)?.path;
    let my_candidate = open_candidate(conn, track_id)?;

    for other in likely_matches(conn, track_id, my_fp.as_ref())? {
        if already_decided(conn, &other, track_id)? {
            continue;
        }
        let evidence = evidence_for(conn, analyzer, my_fp.as_ref(), Path::new(&my_path), &other)?;
        let decision = decide(
            &side_for_track(conn, &other)?,
            &side_for_track(conn, track_id)?,
            &evidence,
            t,
        );
        let other_candidate = open_candidate(conn, &other)?;

        // A discovery candidate that is something the user already owns is
        // held back, not merged into their library.
        if let Some(c) = &my_candidate {
            if owned(conn, &other)? {
                let applied = match &decision.verdict {
                    Verdict::SameRecording | Verdict::PitchedCopy { .. } => {
                        record_evidence(conn, &other, track_id, &decision, &evidence, "discovery")?;
                        pipeline::set_status(
                            conn,
                            c,
                            CandidateStatus::Blocked,
                            Some(&format!(
                                "Already in your library as {}. Retry to review it anyway.",
                                label(conn, &other)?
                            )),
                            now_ms(),
                        )?;
                        Applied::NoChange
                    }
                    Verdict::DifferentVersion => {
                        record_evidence(conn, &other, track_id, &decision, &evidence, "discovery")?;
                        let work = link_versions(conn, &other, track_id)?;
                        explain(
                            conn,
                            c,
                            &format!(
                                "Another version of {}, which is in your library.",
                                label(conn, &other)?
                            ),
                        )?;
                        Applied::LinkedAsVersions { work }
                    }
                    _ => apply(conn, &other, track_id, &decision, &evidence, "discovery")?,
                };
                outcomes.push(MatchOutcome {
                    other,
                    decision,
                    applied,
                });
                continue;
            }
        }
        // The user imported something that is still an open candidate: the
        // candidate is held back and the library copy left alone.
        if let (None, Some(c)) = (&my_candidate, &other_candidate) {
            if owned(conn, track_id)?
                && matches!(
                    decision.verdict,
                    Verdict::SameRecording | Verdict::PitchedCopy { .. }
                )
            {
                record_evidence(conn, &other, track_id, &decision, &evidence, "library")?;
                pipeline::set_status(
                    conn,
                    c,
                    CandidateStatus::Blocked,
                    Some(&format!("Now in your library as {}.", label(conn, track_id)?)),
                    now_ms(),
                )?;
                outcomes.push(MatchOutcome {
                    other,
                    decision,
                    applied: Applied::NoChange,
                });
                continue;
            }
        }
        // Two tracks still in the discovery pipeline are not merged either.
        if my_candidate.is_some() && other_candidate.is_some() {
            record_evidence(conn, &other, track_id, &decision, &evidence, "discovery")?;
            continue;
        }
        let applied = apply(conn, &other, track_id, &decision, &evidence, "library")?;
        let merged = matches!(
            applied,
            Applied::Merged { .. } | Applied::MergedAsPitchedCopy { .. }
        );
        outcomes.push(MatchOutcome {
            other,
            decision,
            applied,
        });
        if merged {
            break;
        }
    }
    Ok(outcomes)
}

/// Whether a candidate may be marked ready: not when matching held it back.
pub fn candidate_is_active(conn: &Connection, candidate_id: &str) -> Result<bool> {
    let s = pipeline::state(conn, candidate_id)?;
    Ok(s.status == CandidateStatus::Active && s.stage != Stage::Reviewed)
}
