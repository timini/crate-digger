//! Worker threads that run jobs claimed from the scheduler.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use super::scheduler::Scheduler;
use super::{Job, JobError};
use crate::adapters::AdapterError;

/// Runs one kind of job.
pub trait Handler: Send + Sync {
    fn kind(&self) -> &'static str;
    fn run(&self, ctx: &mut JobCtx<'_>) -> Result<(), JobError>;
}

/// What a handler can see and do while it runs.
pub struct JobCtx<'a> {
    pub conn: &'a Connection,
    pub job: &'a Job,
    owner: &'a str,
    scheduler: &'a Scheduler,
    stop: &'a AtomicBool,
}

impl<'a> JobCtx<'a> {
    pub fn new(
        conn: &'a Connection,
        job: &'a Job,
        owner: &'a str,
        scheduler: &'a Scheduler,
        stop: &'a AtomicBool,
    ) -> Self {
        JobCtx {
            conn,
            job,
            owner,
            scheduler,
            stop,
        }
    }

    pub fn now(&self) -> i64 {
        self.scheduler.now()
    }

    pub fn payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, JobError> {
        self.job
            .payload_as()
            .map_err(|e| JobError::Fatal(format!("bad job payload: {e}")))
    }

    /// The last checkpoint saved by an earlier run of this job, if any.
    pub fn checkpoint<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        self.job
            .checkpoint
            .as_ref()
            .and_then(|c| serde_json::from_value(c.clone()).ok())
    }

    /// Persist progress. Fails with `Stopped` if the app is quitting or the
    /// job was paused, cancelled or taken over.
    pub fn save_checkpoint<T: serde::Serialize>(&mut self, value: &T) -> Result<(), JobError> {
        self.check_stop()?;
        let v = serde_json::to_value(value).map_err(|e| JobError::Fatal(e.to_string()))?;
        super::save_checkpoint(self.conn, &self.job.id, self.owner, &v, self.now())
    }

    /// Extend the lease and check whether to stop.
    pub fn heartbeat(&self) -> Result<(), JobError> {
        self.check_stop()?;
        super::heartbeat(
            self.conn,
            &self.job.id,
            self.owner,
            self.scheduler.lease_ms,
            self.now(),
        )
    }

    pub fn check_stop(&self) -> Result<(), JobError> {
        if self.stop.load(Ordering::SeqCst) {
            Err(JobError::Stopped)
        } else {
            Ok(())
        }
    }

    /// Map an adapter failure onto the job system, attributing
    /// authentication errors to this job's connector.
    pub fn adapter_error(&self, e: AdapterError) -> JobError {
        match e {
            AdapterError::Auth(message) => JobError::Auth {
                connector: self.job.connector.clone().unwrap_or_else(|| "connector".into()),
                message,
            },
            AdapterError::Unavailable(m) => JobError::Retryable(format!("service unavailable: {m}")),
            AdapterError::RateLimited { .. } => JobError::Retryable("rate limited".into()),
            AdapterError::Invalid(m) => JobError::Fatal(m),
        }
    }
}

/// Claim one job, run it and record the outcome. Returns false if there was
/// nothing to do. Used by worker threads and directly by tests.
pub fn run_one(
    conn: &mut Connection,
    scheduler: &Scheduler,
    handlers: &HashMap<&'static str, Arc<dyn Handler>>,
    kinds: &[&str],
    owner: &str,
    stop: &AtomicBool,
) -> crate::Result<bool> {
    let Some(job) = scheduler.claim(conn, owner, kinds)? else {
        return Ok(false);
    };
    let Some(handler) = handlers.get(job.kind.as_str()) else {
        let err = JobError::Fatal(format!("no handler for job kind {}", job.kind));
        super::record_failure(conn, &job, owner, &err, scheduler.now())?;
        return Ok(true);
    };
    let outcome = {
        let mut ctx = JobCtx::new(conn, &job, owner, scheduler, stop);
        std::panic::catch_unwind(AssertUnwindSafe(|| handler.run(&mut ctx)))
    };
    let result = match outcome {
        Ok(r) => r,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!(job = %job.id, kind = %job.kind, "job panicked: {msg}");
            Err(JobError::Retryable(format!("internal error: {msg}")))
        }
    };
    match result {
        Ok(()) => {
            if let Err(JobError::Stopped) = super::complete(conn, &job.id, owner, scheduler.now()) {
                tracing::info!(job = %job.id, "job finished after losing its lease; result kept by checkpoint");
            }
        }
        Err(err) => {
            if !matches!(err, JobError::Stopped) {
                tracing::warn!(job = %job.id, kind = %job.kind, "job did not complete: {err}");
            }
            super::record_failure(conn, &job, owner, &err, scheduler.now())?;
        }
    }
    Ok(true)
}

type Wake = Arc<(Mutex<u64>, Condvar)>;

pub struct WorkerPool {
    threads: Vec<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    wake: Wake,
}

impl WorkerPool {
    /// Start `threads` workers, each with its own database connection.
    pub fn start(
        db_path: PathBuf,
        scheduler: Arc<Scheduler>,
        handlers: Vec<Arc<dyn Handler>>,
        threads: usize,
    ) -> crate::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let wake: Wake = Arc::new((Mutex::new(0), Condvar::new()));
        let handlers: Arc<HashMap<&'static str, Arc<dyn Handler>>> =
            Arc::new(handlers.into_iter().map(|h| (h.kind(), h)).collect());
        let mut joins = Vec::new();
        for n in 0..threads {
            let mut conn = crate::db::open_existing(&db_path)?;
            let scheduler = scheduler.clone();
            let handlers = handlers.clone();
            let stop = stop.clone();
            let wake = wake.clone();
            let owner = format!("worker-{n}-{}", crate::util::new_id());
            let join = std::thread::Builder::new()
                .name(format!("cd-worker-{n}"))
                .spawn(move || {
                    let mut kinds: Vec<&str> = handlers.keys().copied().collect();
                    kinds.sort();
                    let mut rotation = n;
                    while !stop.load(Ordering::SeqCst) {
                        // Rotate the kind order so no kind starves another.
                        rotation = (rotation + 1) % kinds.len().max(1);
                        let mut order = kinds.clone();
                        order.rotate_left(rotation);
                        let _ = super::requeue_expired_leases(&conn, scheduler.now());
                        match run_one(&mut conn, &scheduler, &handlers, &order, &owner, &stop) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(e) => tracing::error!("worker error: {e}"),
                        }
                        let (lock, cvar) = &*wake;
                        let guard = lock.lock().unwrap();
                        let _ = cvar.wait_timeout(guard, Duration::from_millis(500)).unwrap();
                    }
                })
                .expect("spawn worker thread");
            joins.push(join);
        }
        Ok(WorkerPool {
            threads: joins,
            stop,
            wake,
        })
    }

    /// Wake idle workers, for example after enqueueing work.
    pub fn notify(&self) {
        let (lock, cvar) = &*self.wake;
        *lock.lock().unwrap() += 1;
        cvar.notify_all();
    }

    /// Stop all workers and wait for them. Running handlers see `Stopped` at
    /// their next checkpoint; their jobs stay `running` for the caller to
    /// park with [`super::park_for_quit`].
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.notify();
        for t in self.threads {
            let _ = t.join();
        }
    }
}
