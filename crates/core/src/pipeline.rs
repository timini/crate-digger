//! Candidate pipeline:
//! `Candidate, Identified, Acquisition queued, Downloading, Validating,
//! Analysing, Ready, Reviewed`.
//!
//! Every stage change goes through [`transition`], which rejects moves the
//! spec does not allow. Progress (`stage`) is separate from whether the
//! candidate is currently moving (`status`); a non-active status always
//! carries a reason.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::domain::{CandidateStatus, Stage};
use crate::{Error, Result};

/// Whether `from -> to` is a permitted stage change.
pub fn allowed(from: Stage, to: Stage) -> bool {
    use Stage::*;
    matches!(
        (from, to),
        (Candidate, Identified)
            | (Identified, AcquisitionQueued)
            // Bypass: a matching local file already exists.
            | (Identified, Validating)
            | (AcquisitionQueued, Downloading)
            | (Downloading, Validating)
            // A transfer or validation failed; try another source.
            | (Downloading, AcquisitionQueued)
            | (Validating, AcquisitionQueued)
            | (Validating, Analysing)
            // Bypass: compatible shared features already exist.
            | (Validating, Ready)
            | (Analysing, Ready)
            | (Ready, Reviewed)
            // Undoing the only review returns the track to the queue.
            | (Reviewed, Ready)
    )
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateState {
    pub stage: Stage,
    pub status: CandidateStatus,
    pub status_reason: Option<String>,
}

pub fn state(conn: &Connection, candidate_id: &str) -> Result<CandidateState> {
    conn.query_row(
        "SELECT stage, status, status_reason FROM candidate WHERE id = ?1",
        params![candidate_id],
        |r| {
            Ok(CandidateState {
                stage: r.get(0)?,
                status: r.get(1)?,
                status_reason: r.get(2)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("candidate {candidate_id}")))
}

/// Move a candidate to `to`. Moving forward also makes it active again.
/// Moving to the stage it is already in is a no-op, so retried jobs can call
/// this safely.
pub fn transition(conn: &Connection, candidate_id: &str, to: Stage, now: i64) -> Result<()> {
    let current = state(conn, candidate_id)?;
    if current.stage == to {
        return Ok(());
    }
    if !allowed(current.stage, to) {
        return Err(Error::Invalid(format!(
            "candidate cannot move from {} to {}",
            current.stage, to
        )));
    }
    conn.execute(
        "UPDATE candidate SET stage = ?2, status = 'active', status_reason = NULL, updated_at = ?3
         WHERE id = ?1",
        params![candidate_id, to, now],
    )?;
    Ok(())
}

pub fn set_status(
    conn: &Connection,
    candidate_id: &str,
    status: CandidateStatus,
    reason: Option<&str>,
    now: i64,
) -> Result<()> {
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    if status != CandidateStatus::Active && reason.is_none() {
        return Err(Error::Invalid(format!("a {status} candidate needs a reason")));
    }
    let n = conn.execute(
        "UPDATE candidate SET status = ?2, status_reason = ?3, updated_at = ?4 WHERE id = ?1",
        params![candidate_id, status, reason, now],
    )?;
    if n == 0 {
        return Err(Error::NotFound(format!("candidate {candidate_id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::meta::create_track;

    fn candidate(conn: &Connection) -> String {
        let t = create_track(conn).unwrap();
        let id = crate::util::new_id();
        conn.execute(
            "INSERT INTO candidate (id, track_id, stage, created_at, updated_at)
             VALUES (?1, ?2, 'candidate', 0, 0)",
            params![id, t],
        )
        .unwrap();
        id
    }

    #[test]
    fn full_pipeline_path_is_allowed() {
        let conn = open_in_memory().unwrap();
        let c = candidate(&conn);
        for s in [
            Stage::Identified,
            Stage::AcquisitionQueued,
            Stage::Downloading,
            Stage::Validating,
            Stage::Analysing,
            Stage::Ready,
            Stage::Reviewed,
        ] {
            transition(&conn, &c, s, 1).unwrap();
        }
        assert_eq!(state(&conn, &c).unwrap().stage, Stage::Reviewed);
    }

    #[test]
    fn local_file_and_shared_feature_bypasses() {
        let conn = open_in_memory().unwrap();
        let c = candidate(&conn);
        transition(&conn, &c, Stage::Identified, 1).unwrap();
        transition(&conn, &c, Stage::Validating, 1).unwrap();
        transition(&conn, &c, Stage::Ready, 1).unwrap();
    }

    #[test]
    fn skipping_identification_or_validation_is_rejected() {
        let conn = open_in_memory().unwrap();
        let c = candidate(&conn);
        assert!(transition(&conn, &c, Stage::AcquisitionQueued, 1).is_err());
        assert!(transition(&conn, &c, Stage::Ready, 1).is_err());
        transition(&conn, &c, Stage::Identified, 1).unwrap();
        transition(&conn, &c, Stage::AcquisitionQueued, 1).unwrap();
        transition(&conn, &c, Stage::Downloading, 1).unwrap();
        // Downloaded audio must be validated before it can be ready.
        assert!(transition(&conn, &c, Stage::Ready, 1).is_err());
    }

    #[test]
    fn non_active_status_requires_reason() {
        let conn = open_in_memory().unwrap();
        let c = candidate(&conn);
        assert!(set_status(&conn, &c, CandidateStatus::Blocked, None, 1).is_err());
        assert!(set_status(&conn, &c, CandidateStatus::Blocked, Some("  "), 1).is_err());
        set_status(
            &conn,
            &c,
            CandidateStatus::Blocked,
            Some("Needs review: two mixes match"),
            1,
        )
        .unwrap();
        let s = state(&conn, &c).unwrap();
        assert_eq!(s.status, CandidateStatus::Blocked);
        assert!(s.status_reason.unwrap().contains("two mixes"));
        // The database enforces it too.
        let raw = conn.execute(
            "UPDATE candidate SET status = 'failed', status_reason = NULL WHERE id = ?1",
            params![c],
        );
        assert!(raw.is_err());
    }

    #[test]
    fn moving_forward_clears_a_failure() {
        let conn = open_in_memory().unwrap();
        let c = candidate(&conn);
        set_status(&conn, &c, CandidateStatus::Failed, Some("Lookup failed"), 1).unwrap();
        transition(&conn, &c, Stage::Identified, 2).unwrap();
        assert_eq!(state(&conn, &c).unwrap().status, CandidateStatus::Active);
    }
}
