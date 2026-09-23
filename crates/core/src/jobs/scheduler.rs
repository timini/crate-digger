//! The scheduler decides which job runs next. It is the only place that
//! enforces concurrency, daily and storage limits, so adapters cannot get
//! around them: workers can only obtain work through [`Scheduler::claim`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{kinds, Job, JOB_COLUMNS};
use crate::util::{now_ms, utc_day};
use crate::Result;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum running jobs per kind. Kinds not listed use `default_concurrency`.
    pub concurrency: HashMap<String, usize>,
    pub default_concurrency: usize,
    /// Maximum new jobs started per UTC day, per kind.
    pub daily: HashMap<String, u32>,
    /// Kinds that stop when temporary audio reaches `staging_budget_bytes`.
    pub storage_kinds: Vec<String>,
    pub staging_budget_bytes: u64,
    /// Kinds whose concurrency drops to `concurrency_during_playback`
    /// while audio is playing.
    pub yield_to_playback: Vec<String>,
    pub concurrency_during_playback: usize,
}

impl Default for Limits {
    /// Defaults from the product spec.
    fn default() -> Self {
        Limits {
            concurrency: HashMap::from([
                (kinds::ACQUIRE.to_string(), 2),
                (kinds::ANALYSE.to_string(), 1),
                (kinds::IMPORT.to_string(), 1),
            ]),
            default_concurrency: 1,
            daily: HashMap::from([(kinds::ACQUIRE.to_string(), 100)]),
            storage_kinds: vec![kinds::ACQUIRE.to_string()],
            staging_budget_bytes: 10 * 1024 * 1024 * 1024,
            yield_to_playback: vec![kinds::ANALYSE.to_string()],
            concurrency_during_playback: 1,
        }
    }
}

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
/// Bytes of temporary audio currently held, measured inside the scheduler's
/// transaction.
pub type UsageProbe = Arc<dyn Fn(&Connection) -> u64 + Send + Sync>;

/// Staging usage as recorded in the database: every registered staged file.
pub fn staged_bytes(conn: &Connection) -> u64 {
    conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM audio_file WHERE origin = 'staged'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u64)
    .unwrap_or(0)
}

pub struct Scheduler {
    limits: RwLock<Limits>,
    playback_active: AtomicBool,
    accepting: AtomicBool,
    staging_usage: UsageProbe,
    clock: Clock,
    pub lease_ms: i64,
}

impl Scheduler {
    pub fn new(limits: Limits, staging_usage: UsageProbe) -> Self {
        Self::with_clock(limits, staging_usage, Arc::new(now_ms))
    }

    pub fn with_clock(limits: Limits, staging_usage: UsageProbe, clock: Clock) -> Self {
        Scheduler {
            limits: RwLock::new(limits),
            playback_active: AtomicBool::new(false),
            accepting: AtomicBool::new(true),
            staging_usage,
            clock,
            lease_ms: 5 * 60 * 1000,
        }
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn limits(&self) -> Limits {
        self.limits.read().unwrap().clone()
    }

    pub fn set_limits(&self, limits: Limits) {
        *self.limits.write().unwrap() = limits;
    }

    pub fn set_playback_active(&self, active: bool) {
        self.playback_active.store(active, Ordering::SeqCst);
    }

    pub fn playback_active(&self) -> bool {
        self.playback_active.load(Ordering::SeqCst)
    }

    /// Stop handing out work (used on Quit).
    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::SeqCst);
    }

    fn concurrency_for(&self, limits: &Limits, kind: &str) -> usize {
        let base = limits
            .concurrency
            .get(kind)
            .copied()
            .unwrap_or(limits.default_concurrency);
        if self.playback_active() && limits.yield_to_playback.iter().any(|k| k == kind) {
            base.min(limits.concurrency_during_playback)
        } else {
            base
        }
    }

    /// Take the next runnable job of one of `kinds`, or `None` if nothing is
    /// runnable right now. Kinds are tried in the order given.
    pub fn claim(&self, conn: &mut Connection, owner: &str, kinds: &[&str]) -> Result<Option<Job>> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let now = self.now();
        let limits = self.limits();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let used = (self.staging_usage)(&tx);
        enforce_budgets(&tx, &limits, used, now)?;

        for kind in kinds {
            let running: i64 = tx.query_row(
                "SELECT COUNT(*) FROM job WHERE kind = ?1 AND state = 'running'",
                params![kind],
                |r| r.get(0),
            )?;
            if running as usize >= self.concurrency_for(&limits, kind) {
                continue;
            }
            let job = tx
                .query_row(
                    &format!(
                        "SELECT {JOB_COLUMNS} FROM job
                         WHERE kind = ?1 AND state = 'queued' AND next_run_at <= ?2
                           AND (connector IS NULL OR connector NOT IN
                                (SELECT connector FROM connector_state WHERE status = 'auth_failed'))
                         ORDER BY next_run_at, created_at LIMIT 1"
                    ),
                    params![kind, now],
                    Job::from_row,
                )
                .optional()?;
            let Some(job) = job else { continue };

            // Only a job's first start counts against the daily limit.
            if job.attempts == 0 {
                if let Some(limit) = limits.daily.get(*kind) {
                    let day = utc_day(now);
                    let used = daily_count(&tx, &day, kind)?;
                    if used >= *limit as i64 {
                        continue;
                    }
                    tx.execute(
                        "INSERT INTO daily_counter (day, kind, count) VALUES (?1, ?2, 1)
                         ON CONFLICT (day, kind) DO UPDATE SET count = count + 1",
                        params![day, kind],
                    )?;
                }
            }

            tx.execute(
                "UPDATE job SET state = 'running', attempts = attempts + 1, lease_owner = ?2,
                                lease_expires_at = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![job.id, owner, now + self.lease_ms, now],
            )?;
            tx.commit()?;
            return Ok(Some(super::get(conn, &job.id)?));
        }
        tx.commit()?;
        Ok(None)
    }

    /// Apply limits without claiming anything, so blocked reasons show up in
    /// the activity view promptly.
    pub fn refresh(&self, conn: &Connection) -> Result<()> {
        enforce_budgets(conn, &self.limits(), (self.staging_usage)(conn), self.now())
    }

    pub fn status(&self, conn: &Connection) -> Result<SchedulerStatus> {
        let limits = self.limits();
        let now = self.now();
        let day = utc_day(now);
        let mut daily = Vec::new();
        for (kind, limit) in &limits.daily {
            daily.push(DailyUsage {
                kind: kind.clone(),
                used: daily_count(conn, &day, kind)?,
                limit: *limit as i64,
            });
        }
        daily.sort_by(|a, b| a.kind.cmp(&b.kind));
        Ok(SchedulerStatus {
            accepting: self.accepting.load(Ordering::SeqCst),
            playback_active: self.playback_active(),
            staging_used_bytes: (self.staging_usage)(conn),
            staging_budget_bytes: limits.staging_budget_bytes,
            daily,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DailyUsage {
    pub kind: String,
    pub used: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchedulerStatus {
    pub accepting: bool,
    pub playback_active: bool,
    pub staging_used_bytes: u64,
    pub staging_budget_bytes: u64,
    pub daily: Vec<DailyUsage>,
}

fn daily_count(conn: &Connection, day: &str, kind: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT count FROM daily_counter WHERE day = ?1 AND kind = ?2",
            params![day, kind],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Block or unblock queued jobs according to the daily and storage limits.
fn enforce_budgets(conn: &Connection, limits: &Limits, staging_used: u64, now: i64) -> Result<()> {
    let day = utc_day(now);
    for (kind, limit) in &limits.daily {
        let used = daily_count(conn, &day, kind)?;
        if used >= *limit as i64 {
            let reason = format!(
                "Daily limit of {limit} new {kind} jobs reached. More start after midnight UTC, \
                 or raise the limit in Settings."
            );
            conn.execute(
                "UPDATE job SET state = 'blocked', hold_code = 'daily_limit', reason = ?2, updated_at = ?3
                 WHERE kind = ?1 AND state = 'queued' AND attempts = 0",
                params![kind, reason, now],
            )?;
        } else {
            unblock(conn, kind, "daily_limit", now)?;
        }
    }
    let over_budget = staging_used >= limits.staging_budget_bytes;
    for kind in &limits.storage_kinds {
        if over_budget {
            let reason = format!(
                "Temporary audio is using {} of its {} budget. Review, keep or clear temporary \
                 tracks to continue.",
                format_bytes(staging_used),
                format_bytes(limits.staging_budget_bytes)
            );
            conn.execute(
                "UPDATE job SET state = 'blocked', hold_code = 'storage_limit', reason = ?2, updated_at = ?3
                 WHERE kind = ?1 AND state = 'queued'",
                params![kind, reason, now],
            )?;
        } else {
            unblock(conn, kind, "storage_limit", now)?;
        }
    }
    Ok(())
}

fn unblock(conn: &Connection, kind: &str, hold: &str, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE job SET state = 'queued', hold_code = NULL, reason = NULL, updated_at = ?3
         WHERE kind = ?1 AND state = 'blocked' AND hold_code = ?2",
        params![kind, hold, now],
    )?;
    Ok(())
}
