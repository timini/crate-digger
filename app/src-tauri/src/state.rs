use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use cd_core::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use cd_core::jobs::worker::{Handler, WorkerPool};
use rusqlite::Connection;

pub struct AppState {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    db: Mutex<Connection>,
    pub scheduler: Arc<Scheduler>,
    pool: Mutex<Option<WorkerPool>>,
}

impl AppState {
    pub fn open(data_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("crate-digger.sqlite");
        let conn = cd_core::db::open(&db_path)?;
        tracing::info!(path = %db_path.display(), "opened database");

        let recovery = cd_core::jobs::recover_on_start(&conn, cd_core::util::now_ms())?;
        if recovery != Default::default() {
            tracing::info!(?recovery, "resumed background jobs");
        }
        let scheduler = Arc::new(Scheduler::new(Limits::default(), Arc::new(staged_bytes)));
        Ok(AppState {
            data_dir: data_dir.to_path_buf(),
            db_path,
            db: Mutex::new(conn),
            scheduler,
            pool: Mutex::new(None),
        })
    }

    pub fn db(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.db.lock().map_err(|_| "database lock poisoned".to_string())
    }

    pub fn start_workers(&self, handlers: Vec<Arc<dyn Handler>>) -> Result<(), String> {
        let pool = WorkerPool::start(self.db_path.clone(), self.scheduler.clone(), handlers, 3)
            .map_err(|e| e.to_string())?;
        *self.pool.lock().unwrap() = Some(pool);
        Ok(())
    }

    /// Wake idle workers after new work was queued.
    pub fn notify_workers(&self) {
        if let Some(pool) = &*self.pool.lock().unwrap() {
            pool.notify();
        }
    }

    /// Explicit Quit: stop handing out work, let workers stop at their next
    /// checkpoint, then record running jobs as resumable.
    pub fn shutdown(&self) {
        self.scheduler.stop_accepting();
        if let Some(pool) = self.pool.lock().unwrap().take() {
            pool.stop();
        }
        if let Ok(conn) = self.db() {
            match cd_core::jobs::park_for_quit(&conn, cd_core::util::now_ms()) {
                Ok(n) => tracing::info!(parked = n, "stopped background work"),
                Err(e) => tracing::error!("could not park jobs on quit: {e}"),
            }
        }
    }
}
