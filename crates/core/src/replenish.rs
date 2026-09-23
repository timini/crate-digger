//! Keeping a queue of tracks ready to hear.
//!
//! When fewer than `replenish_below` tracks are ready, discovery is asked
//! for enough candidates to reach `ready_target`, counting work already in
//! progress. The scheduler's limits still apply to every step. `health`
//! explains why the queue is short, and `availability` reports how often it
//! has not been.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::settings::UserLimits;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Buffer {
    pub ready: i64,
    pub in_progress: i64,
    pub target: i64,
    pub below: i64,
}

pub fn buffer(conn: &Connection, limits: &UserLimits) -> Result<Buffer> {
    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(Buffer {
        ready: count(
            "SELECT COUNT(*) FROM candidate c WHERE c.stage = 'ready' AND c.status = 'active'
               AND EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = c.track_id AND f.availability = 'available')",
        )?,
        in_progress: count(
            "SELECT COUNT(*) FROM candidate WHERE status = 'active'
               AND stage IN ('candidate', 'identified', 'acquisition_queued', 'downloading', 'validating', 'analysing')",
        )?,
        target: limits.ready_target as i64,
        below: limits.replenish_below as i64,
    })
}

/// How many new candidates to ask for now, or 0.
pub fn shortfall(b: &Buffer) -> i64 {
    if b.ready >= b.below {
        return 0;
    }
    (b.target - b.ready - b.in_progress).max(0)
}

/// Queue discovery for the shortfall unless a discovery run is already waiting.
pub fn top_up(conn: &Connection, connector: &str, limits: &UserLimits, now: i64) -> Result<Option<String>> {
    let wanted = shortfall(&buffer(conn, limits)?);
    if wanted == 0 {
        return Ok(None);
    }
    let pending: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM job WHERE kind = 'discover' AND state IN ('queued', 'running', 'blocked', 'paused'))",
        [],
        |r| r.get(0),
    )?;
    if pending {
        return Ok(None);
    }
    // Discovery returns candidates, not ready tracks; ask for a batch with headroom.
    let limit = (wanted as usize).clamp(5, 50);
    crate::discovery::request_discovery(conn, connector, limit, now).map(Some)
}

/// Why the queue is short, most important first. Each has a code the UI
/// can tell apart and a sentence for the user.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hold {
    /// `auth`, `unavailable`, `daily_limit`, `storage_limit`, `sources_failed`,
    /// `no_candidates`, `needs_choice`, `in_progress`.
    pub code: String,
    pub message: String,
}

pub fn health(conn: &Connection) -> Result<Vec<Hold>> {
    let mut out = vec![];
    let push = |out: &mut Vec<Hold>, code: &str, message: String| {
        out.push(Hold {
            code: code.into(),
            message,
        })
    };
    for c in crate::jobs::connector_health(conn)? {
        let reason = c.reason.unwrap_or_default();
        match c.status {
            crate::jobs::ConnectorStatus::AuthFailed => push(
                &mut out,
                "auth",
                format!("{} needs attention: {reason}", c.connector),
            ),
            crate::jobs::ConnectorStatus::Unavailable => push(
                &mut out,
                "unavailable",
                format!("{} is unavailable: {reason}", c.connector),
            ),
            crate::jobs::ConnectorStatus::Ok => {}
        }
    }
    let held = |code: &str| -> Result<Option<String>> {
        Ok(conn
            .query_row(
                "SELECT reason FROM job WHERE state = 'blocked' AND hold_code = ?1 ORDER BY updated_at DESC LIMIT 1",
                params![code],
                |r| r.get(0),
            )
            .optional()?)
    };
    if let Some(r) = held("daily_limit")? {
        push(&mut out, "daily_limit", r);
    }
    if let Some(r) = held("storage_limit")? {
        push(&mut out, "storage_limit", r);
    }
    if let Some(run) = crate::discovery::recent_runs(conn, 1)?.into_iter().next() {
        match run.outcome.as_str() {
            "failed" => push(
                &mut out,
                "sources_failed",
                format!(
                    "The last discovery run failed: {}",
                    run.detail.unwrap_or_default()
                ),
            ),
            "empty" => push(
                &mut out,
                "no_candidates",
                "The last discovery run found nothing new. Add seeds, rate tracks or read a page.".into(),
            ),
            _ => {}
        }
    }
    let choices: i64 = conn.query_row(
        "SELECT COUNT(*) FROM acquisition_choice WHERE resolved_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    if choices > 0 {
        push(
            &mut out,
            "needs_choice",
            format!("{choices} downloads are waiting for your choice."),
        );
    }
    Ok(out)
}

/// Record the ready count; called about once a minute while the app runs.
pub fn sample(conn: &Connection, ready: i64, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO buffer_sample (at, ready) VALUES (?1, ?2)",
        params![now, ready],
    )?;
    // Keep 30 days.
    conn.execute(
        "DELETE FROM buffer_sample WHERE at < ?1",
        params![now - 30 * 86_400_000],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Availability {
    pub samples: i64,
    /// Share of samples with at least one track ready.
    pub any_ready: f64,
    /// Share of samples at or above the replenishment threshold.
    pub above_threshold: f64,
}

pub fn availability(conn: &Connection, since: i64, threshold: i64) -> Result<Availability> {
    let (samples, any, above): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(ready > 0), 0), COALESCE(SUM(ready >= ?2), 0)
         FROM buffer_sample WHERE at >= ?1",
        params![since, threshold],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let share = |n: i64| {
        if samples == 0 {
            0.0
        } else {
            n as f64 / samples as f64
        }
    };
    Ok(Availability {
        samples,
        any_ready: share(any),
        above_threshold: share(above),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(ready: i64, in_progress: i64) -> Buffer {
        Buffer {
            ready,
            in_progress,
            target: 50,
            below: 30,
        }
    }

    #[test]
    fn tops_up_only_below_the_threshold_and_counts_work_in_progress() {
        assert_eq!(shortfall(&b(30, 0)), 0);
        assert_eq!(shortfall(&b(29, 0)), 21);
        assert_eq!(shortfall(&b(10, 15)), 25);
        assert_eq!(shortfall(&b(10, 45)), 0);
    }

    #[test]
    fn top_up_queues_once_and_holds_are_distinct() {
        let conn = crate::db::open_in_memory().unwrap();
        let limits = UserLimits::default();
        assert!(top_up(&conn, "live", &limits, 1).unwrap().is_some());
        assert!(
            top_up(&conn, "live", &limits, 2).unwrap().is_none(),
            "a run is already waiting"
        );

        crate::jobs::set_connector_status(
            &conn,
            "slskd",
            crate::jobs::ConnectorStatus::AuthFailed,
            Some("bad login"),
            3,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO job (id, kind, state, hold_code, reason, payload, idempotency_key, next_run_at, created_at, updated_at)
             VALUES ('j', 'acquire', 'blocked', 'daily_limit', 'Daily limit of 100 reached.', '{}', 'k', 0, 0, 0)",
            [],
        )
        .unwrap();
        let codes: Vec<String> = health(&conn).unwrap().into_iter().map(|h| h.code).collect();
        assert_eq!(codes, vec!["auth", "daily_limit"]);
    }

    #[test]
    fn availability_is_the_share_of_samples() {
        let conn = crate::db::open_in_memory().unwrap();
        for (t, ready) in [(1, 0), (2, 10), (3, 35), (4, 40)] {
            sample(&conn, ready, t).unwrap();
        }
        let a = availability(&conn, 0, 30).unwrap();
        assert_eq!((a.samples, a.any_ready, a.above_threshold), (4, 0.75, 0.5));
    }
}
