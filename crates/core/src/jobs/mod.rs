//! Durable background jobs.
//!
//! Every job is a row in SQLite. Workers take a time-limited lease on a job
//! before running it, and every update from a worker checks that it still
//! holds the lease. Side effects that must not repeat (starting a transfer,
//! submitting a contribution) are made idempotent with the job's
//! idempotency key and recorded in the job's checkpoint, so a job resumed
//! after a crash continues rather than starting again.

pub mod scheduler;
pub mod worker;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::domain::{HoldCode, JobState};
use crate::util::new_id;
use crate::{Error, Result};

pub mod kinds {
    pub const IMPORT: &str = "import";
    pub const ACQUIRE: &str = "acquire";
    pub const VALIDATE: &str = "validate";
    pub const ANALYSE: &str = "analyse";
    pub const WAVEFORM: &str = "waveform";
    pub const DISCOVER: &str = "discover";
    pub const SYNC: &str = "sync";
    pub const ARCHIVE: &str = "archive";
    pub const YOUTUBE: &str = "youtube";
    pub const METADATA: &str = "metadata";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub connector: Option<String>,
    pub state: JobState,
    pub hold_code: Option<HoldCode>,
    pub reason: Option<String>,
    pub payload: serde_json::Value,
    pub checkpoint: Option<serde_json::Value>,
    pub idempotency_key: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub next_run_at: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const JOB_COLUMNS: &str = "id, kind, connector, state, hold_code, reason, payload, checkpoint,
    idempotency_key, attempts, max_attempts, next_run_at, lease_owner, lease_expires_at,
    created_at, updated_at";

impl Job {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        let json = |i: usize, r: &Row<'_>| -> rusqlite::Result<Option<serde_json::Value>> {
            let s: Option<String> = r.get(i)?;
            s.map(|s| {
                serde_json::from_str(&s).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
                })
            })
            .transpose()
        };
        Ok(Job {
            id: r.get(0)?,
            kind: r.get(1)?,
            connector: r.get(2)?,
            state: r.get(3)?,
            hold_code: r.get(4)?,
            reason: r.get(5)?,
            payload: json(6, r)?.unwrap_or(serde_json::Value::Null),
            checkpoint: json(7, r)?,
            idempotency_key: r.get(8)?,
            attempts: r.get(9)?,
            max_attempts: r.get(10)?,
            next_run_at: r.get(11)?,
            lease_owner: r.get(12)?,
            lease_expires_at: r.get(13)?,
            created_at: r.get(14)?,
            updated_at: r.get(15)?,
        })
    }

    pub fn payload_as<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_value(self.payload.clone())?)
    }
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: String,
    pub connector: Option<String>,
    pub payload: serde_json::Value,
    pub idempotency_key: String,
    pub max_attempts: i64,
}

impl NewJob {
    pub fn new(kind: &str, idempotency_key: impl Into<String>, payload: serde_json::Value) -> Self {
        NewJob {
            kind: kind.to_string(),
            connector: None,
            payload,
            idempotency_key: idempotency_key.into(),
            max_attempts: 5,
        }
    }

    pub fn connector(mut self, connector: &str) -> Self {
        self.connector = Some(connector.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Enqueued {
    pub id: String,
    /// False when a job with the same idempotency key already existed.
    pub created: bool,
}

/// How a job run ended, other than success.
#[derive(Debug, Clone, PartialEq)]
pub enum JobError {
    /// Temporary problem. Retried with backoff until `max_attempts`.
    Retryable(String),
    /// Credentials rejected. Pauses every job for the connector.
    Auth { connector: String, message: String },
    /// Will not succeed if retried.
    Fatal(String),
    /// A resource limit stops the job until the limit clears.
    Blocked { hold: HoldCode, reason: String },
    /// Nothing is wrong, but the job must wait for something outside the app
    /// (a download queued on another user's computer). Runs again after
    /// `delay_ms` and does not use up an attempt.
    Wait { reason: String, delay_ms: i64 },
    /// The worker lost its lease or is shutting down. The job row is left
    /// for whoever now owns it (or for recovery on the next start).
    Stopped,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Retryable(m) | JobError::Fatal(m) => f.write_str(m),
            JobError::Auth { connector, message } => write!(f, "{connector}: {message}"),
            JobError::Blocked { reason, .. } | JobError::Wait { reason, .. } => f.write_str(reason),
            JobError::Stopped => f.write_str("stopped"),
        }
    }
}

impl From<Error> for JobError {
    fn from(e: Error) -> Self {
        match e {
            // Database or file errors are usually transient (locked DB, a
            // disk briefly unavailable). Retry a bounded number of times.
            Error::Db(_) | Error::Io { .. } => JobError::Retryable(e.to_string()),
            _ => JobError::Fatal(e.to_string()),
        }
    }
}

/// Delay before retry number `attempt` (1-based): 2 s doubling, capped at 10 min.
pub fn backoff_ms(attempt: i64) -> i64 {
    let exp = (attempt.max(1) - 1).min(20) as u32;
    (2_000i64.saturating_mul(1 << exp)).min(600_000)
}

pub fn enqueue(conn: &Connection, job: &NewJob, now: i64) -> Result<Enqueued> {
    let id = new_id();
    let inserted = conn.execute(
        "INSERT INTO job (id, kind, connector, state, payload, idempotency_key, max_attempts,
                          next_run_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?7, ?7)
         ON CONFLICT (idempotency_key) DO NOTHING",
        params![
            id,
            job.kind,
            job.connector,
            job.payload.to_string(),
            job.idempotency_key,
            job.max_attempts,
            now
        ],
    )?;
    if inserted == 1 {
        return Ok(Enqueued { id, created: true });
    }
    let existing: String = conn.query_row(
        "SELECT id FROM job WHERE idempotency_key = ?1",
        params![job.idempotency_key],
        |r| r.get(0),
    )?;
    Ok(Enqueued {
        id: existing,
        created: false,
    })
}

pub fn get(conn: &Connection, id: &str) -> Result<Job> {
    conn.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM job WHERE id = ?1"),
        params![id],
        Job::from_row,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("job {id}")))
}

/// Jobs for the activity view, most recently updated first.
pub fn list(conn: &Connection, states: &[JobState], limit: i64) -> Result<Vec<Job>> {
    let filter = if states.is_empty() {
        String::new()
    } else {
        let list = states
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("WHERE state IN ({list})")
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {JOB_COLUMNS} FROM job {filter} ORDER BY updated_at DESC, id DESC LIMIT ?1"
    ))?;
    let jobs = stmt
        .query_map(params![limit], Job::from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(jobs)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobCount {
    pub kind: String,
    pub state: JobState,
    pub count: i64,
}

pub fn counts(conn: &Connection) -> Result<Vec<JobCount>> {
    let mut stmt =
        conn.prepare("SELECT kind, state, COUNT(*) FROM job GROUP BY kind, state ORDER BY kind, state")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(JobCount {
                kind: r.get(0)?,
                state: r.get(1)?,
                count: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Run `sql` against a job only while `owner` still holds its lease.
/// Returns `JobError::Stopped` if the lease was lost.
fn leased_update(
    conn: &Connection,
    id: &str,
    owner: &str,
    sql: &str,
    extra: &[&dyn rusqlite::ToSql],
) -> std::result::Result<(), JobError> {
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&id, &owner];
    args.extend_from_slice(extra);
    let n = conn
        .execute(
            &format!("{sql} WHERE id = ?1 AND lease_owner = ?2 AND state = 'running'"),
            args.as_slice(),
        )
        .map_err(|e| JobError::Retryable(e.to_string()))?;
    if n == 1 {
        Ok(())
    } else {
        Err(JobError::Stopped)
    }
}

pub fn save_checkpoint(
    conn: &Connection,
    id: &str,
    owner: &str,
    checkpoint: &serde_json::Value,
    now: i64,
) -> std::result::Result<(), JobError> {
    leased_update(
        conn,
        id,
        owner,
        "UPDATE job SET checkpoint = ?3, updated_at = ?4",
        &[&checkpoint.to_string(), &now],
    )
}

pub fn heartbeat(
    conn: &Connection,
    id: &str,
    owner: &str,
    lease_ms: i64,
    now: i64,
) -> std::result::Result<(), JobError> {
    leased_update(
        conn,
        id,
        owner,
        "UPDATE job SET lease_expires_at = ?3",
        &[&(now + lease_ms)],
    )
}

pub fn complete(conn: &Connection, id: &str, owner: &str, now: i64) -> std::result::Result<(), JobError> {
    leased_update(
        conn,
        id,
        owner,
        "UPDATE job SET state = 'done', reason = NULL, hold_code = NULL, lease_owner = NULL,
                        lease_expires_at = NULL, updated_at = ?3",
        &[&now],
    )
}

/// Apply a failed run's outcome. Returns the job's new state, or `None` if
/// the worker no longer held the lease.
pub fn record_failure(
    conn: &Connection,
    job: &Job,
    owner: &str,
    err: &JobError,
    now: i64,
) -> Result<Option<JobState>> {
    let (state, hold, reason, next_run_at) = match err {
        JobError::Stopped => return Ok(None),
        JobError::Retryable(msg) if job.attempts >= job.max_attempts => (
            JobState::Failed,
            None,
            format!("Gave up after {} attempts. Last error: {msg}", job.attempts),
            now,
        ),
        JobError::Retryable(msg) => {
            let delay = backoff_ms(job.attempts);
            (
                JobState::Queued,
                None,
                format!(
                    "Attempt {} of {} failed ({msg}). Retrying in {} s.",
                    job.attempts,
                    job.max_attempts,
                    delay / 1000
                ),
                now + delay,
            )
        }
        JobError::Fatal(msg) => (JobState::Failed, None, msg.clone(), now),
        JobError::Wait { reason, delay_ms } => {
            conn.execute(
                "UPDATE job SET attempts = MAX(attempts - 1, 0) WHERE id = ?1 AND lease_owner = ?2",
                params![job.id, owner],
            )?;
            (JobState::Queued, None, reason.clone(), now + (*delay_ms).max(0))
        }
        JobError::Blocked { hold, reason } => (JobState::Blocked, Some(*hold), reason.clone(), now),
        JobError::Auth { connector, message } => {
            set_connector_status(conn, connector, ConnectorStatus::AuthFailed, Some(message), now)?;
            (
                JobState::Paused,
                Some(HoldCode::ConnectorAuth),
                auth_reason(connector, message),
                now,
            )
        }
    };
    let n = conn.execute(
        "UPDATE job SET state = ?3, hold_code = ?4, reason = ?5, next_run_at = ?6,
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = ?7
         WHERE id = ?1 AND lease_owner = ?2 AND state = 'running'",
        params![job.id, owner, state, hold, reason, next_run_at, now],
    )?;
    Ok((n == 1).then_some(state))
}

fn auth_reason(connector: &str, message: &str) -> String {
    format!("Paused: {connector} needs attention ({message}). Fix the connection in Settings to resume.")
}

/// Cancel a job in any unfinished state. A running job notices at its next
/// checkpoint and stops.
pub fn cancel(conn: &Connection, id: &str, now: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE job SET state = 'cancelled', hold_code = NULL, reason = 'Cancelled by you',
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = ?2
         WHERE id = ?1 AND state NOT IN ('done', 'cancelled')",
        params![id, now],
    )?;
    if n == 0 {
        get(conn, id)?; // NotFound if it does not exist
    }
    Ok(())
}

/// Put a failed or cancelled job back in the queue with a fresh attempt count.
pub fn retry(conn: &Connection, id: &str, now: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE job SET state = 'queued', hold_code = NULL, reason = NULL, attempts = 0,
                        next_run_at = ?2, updated_at = ?2
         WHERE id = ?1 AND state IN ('failed', 'cancelled')",
        params![id, now],
    )?;
    if n == 0 {
        let job = get(conn, id)?;
        return Err(Error::Invalid(format!(
            "only failed or cancelled jobs can be retried; this one is {}",
            job.state
        )));
    }
    Ok(())
}

/// Pause all queued and running work. Running jobs stop at their next
/// checkpoint and resume from it later.
pub fn pause_all(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE job SET state = 'paused', hold_code = 'user', reason = 'Paused by you',
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = ?1
         WHERE state IN ('queued', 'running')",
        params![now],
    )?)
}

pub fn resume_all(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE job SET state = 'queued', hold_code = NULL, reason = NULL, updated_at = ?1
         WHERE state = 'paused' AND hold_code = 'user'",
        params![now],
    )?)
}

/// Called on explicit Quit after workers have stopped.
pub fn park_for_quit(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE job SET state = 'paused', hold_code = 'quit',
                        reason = 'Stopped when Crate Digger quit. Resumes on next start.',
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = ?1
         WHERE state = 'running'",
        params![now],
    )?)
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Recovery {
    pub resumed_after_quit: usize,
    pub resumed_after_crash: usize,
}

/// Run once at startup, before any worker starts.
pub fn recover_on_start(conn: &Connection, now: i64) -> Result<Recovery> {
    let resumed_after_quit = conn.execute(
        "UPDATE job SET state = 'queued', hold_code = NULL, reason = NULL, updated_at = ?1
         WHERE state = 'paused' AND hold_code = 'quit'",
        params![now],
    )?;
    let resumed_after_crash = conn.execute(
        "UPDATE job SET state = 'queued', hold_code = NULL,
                        reason = 'Resumed after Crate Digger stopped unexpectedly.',
                        lease_owner = NULL, lease_expires_at = NULL, next_run_at = ?1, updated_at = ?1
         WHERE state = 'running'",
        params![now],
    )?;
    Ok(Recovery {
        resumed_after_quit,
        resumed_after_crash,
    })
}

/// Requeue jobs whose worker stopped renewing its lease while the app kept
/// running (for example a hung thread).
pub fn requeue_expired_leases(conn: &Connection, now: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE job SET state = 'queued', reason = 'A worker stopped responding; retrying.',
                        lease_owner = NULL, lease_expires_at = NULL, updated_at = ?1
         WHERE state = 'running' AND lease_expires_at < ?1",
        params![now],
    )?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStatus {
    Ok,
    AuthFailed,
    Unavailable,
}

impl ConnectorStatus {
    fn as_str(self) -> &'static str {
        match self {
            ConnectorStatus::Ok => "ok",
            ConnectorStatus::AuthFailed => "auth_failed",
            ConnectorStatus::Unavailable => "unavailable",
        }
    }
}

/// Record a connector's health. Marking it `Ok` resumes jobs that were
/// paused because its sign-in failed.
pub fn set_connector_status(
    conn: &Connection,
    connector: &str,
    status: ConnectorStatus,
    reason: Option<&str>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO connector_state (connector, status, reason, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (connector) DO UPDATE SET
             status = excluded.status, reason = excluded.reason, updated_at = excluded.updated_at",
        params![connector, status.as_str(), reason, now],
    )?;
    match status {
        ConnectorStatus::AuthFailed => {
            let reason = auth_reason(connector, reason.unwrap_or("no details"));
            conn.execute(
                "UPDATE job SET state = 'paused', hold_code = 'connector_auth', reason = ?2, updated_at = ?3
                 WHERE connector = ?1 AND state = 'queued'",
                params![connector, reason, now],
            )?;
        }
        ConnectorStatus::Ok => {
            conn.execute(
                "UPDATE job SET state = 'queued', hold_code = NULL, reason = NULL, updated_at = ?2
                 WHERE connector = ?1 AND state = 'paused' AND hold_code = 'connector_auth'",
                params![connector, now],
            )?;
        }
        ConnectorStatus::Unavailable => {}
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectorHealth {
    pub connector: String,
    pub status: ConnectorStatus,
    pub reason: Option<String>,
    pub updated_at: i64,
}

pub fn connector_health(conn: &Connection) -> Result<Vec<ConnectorHealth>> {
    let mut stmt =
        conn.prepare("SELECT connector, status, reason, updated_at FROM connector_state ORDER BY connector")?;
    let rows = stmt
        .query_map([], |r| {
            let status: String = r.get(1)?;
            Ok(ConnectorHealth {
                connector: r.get(0)?,
                status: match status.as_str() {
                    "auth_failed" => ConnectorStatus::AuthFailed,
                    "unavailable" => ConnectorStatus::Unavailable,
                    _ => ConnectorStatus::Ok,
                },
                reason: r.get(2)?,
                updated_at: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests;
