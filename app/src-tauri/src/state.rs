use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use cd_audio::{OutputConfig, Player};
use cd_core::analysis::handler::SwitchableAnalyzer;
use cd_core::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use cd_core::jobs::worker::{Handler, WorkerPool};
use rusqlite::Connection;

pub struct AppState {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    db: Mutex<Connection>,
    pub scheduler: Arc<Scheduler>,
    pool: Mutex<Option<WorkerPool>>,
    player: Mutex<Option<Arc<Player>>>,
    /// Track the player has loaded, for the UI.
    pub now_playing: Mutex<Option<String>>,
    /// Identifies this run of the app. Skips last for one session.
    pub session_id: String,
    /// Used when no archive folder has been chosen.
    pub default_archive_dir: PathBuf,
    pub analyzer: Arc<SwitchableAnalyzer>,
    /// OS keychain in the app; tests can substitute an in-memory store.
    pub secrets: Arc<dyn cd_connectors::credentials::SecretStore>,
    /// Non-secret connection settings, shared with the live discovery source.
    pub connections: Arc<RwLock<cd_connectors::config::Connections>>,
}

pub fn archive_dir_setting(conn: &Connection) -> Option<PathBuf> {
    cd_core::settings::get::<String>(conn, cd_core::settings::keys::ARCHIVE_DIR)
        .ok()
        .flatten()
        .map(PathBuf::from)
}

impl AppState {
    pub fn open(data_dir: &Path, default_archive_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("crate-digger.sqlite");
        let conn = cd_core::db::open(&db_path)?;
        tracing::info!(path = %db_path.display(), "opened database");

        let recovery = cd_core::jobs::recover_on_start(&conn, cd_core::util::now_ms())?;
        if recovery != Default::default() {
            tracing::info!(?recovery, "resumed background jobs");
        }
        let scheduler = Arc::new(Scheduler::new(Limits::default(), Arc::new(staged_bytes)));
        let connections = cd_core::settings::get_or(
            &conn,
            crate::commands::connections::CONFIG_KEY,
            cd_connectors::config::Connections::default(),
        )
        .unwrap_or_default();
        let model = crate::models::chosen(&conn, data_dir);
        let analyzer = Arc::new(SwitchableAnalyzer::new(Arc::new(crate::workers::analyzer(model))));
        Ok(AppState {
            data_dir: data_dir.to_path_buf(),
            db_path,
            db: Mutex::new(conn),
            scheduler,
            pool: Mutex::new(None),
            player: Mutex::new(None),
            now_playing: Mutex::new(None),
            session_id: cd_core::util::new_id(),
            default_archive_dir,
            analyzer,
            secrets: Arc::new(cd_connectors::credentials::Keychain),
            connections: Arc::new(RwLock::new(connections)),
        })
    }

    /// The audio player, started on first use so a missing output device
    /// never stops the app from opening. Retries until a device is found.
    pub fn player(&self) -> Result<Arc<Player>, String> {
        let mut slot = self.player.lock().unwrap();
        if let Some(p) = &*slot {
            return Ok(p.clone());
        }
        let p = Arc::new(Player::start(OutputConfig::Device)?);
        let volume = self
            .db()
            .ok()
            .and_then(|c| {
                cd_core::settings::get::<f32>(&c, cd_core::settings::keys::VOLUME)
                    .ok()
                    .flatten()
            })
            .unwrap_or(1.0);
        p.set_volume(volume);
        *slot = Some(p.clone());
        Ok(p)
    }

    pub fn player_if_started(&self) -> Option<Arc<Player>> {
        self.player.lock().unwrap().clone()
    }

    pub fn db(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.db.lock().map_err(|_| "database lock poisoned".to_string())
    }

    /// Where downloads wait until they are kept or cleared.
    pub fn staging_dir(&self) -> PathBuf {
        self.db()
            .ok()
            .and_then(|c| {
                cd_core::settings::get::<String>(&c, cd_core::settings::keys::STAGING_DIR)
                    .ok()
                    .flatten()
            })
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("staging"))
    }

    /// Load the user's limits into the scheduler.
    pub fn apply_limits(&self) {
        if let Ok(conn) = self.db() {
            if let Ok(l) = cd_core::settings::limits(&conn) {
                self.scheduler.set_limits(l.scheduler_limits());
            }
        }
    }

    pub fn close_to_tray(&self) -> bool {
        self.db()
            .ok()
            .and_then(|c| {
                cd_core::settings::get::<bool>(&c, cd_core::settings::keys::CLOSE_TO_TRAY)
                    .ok()
                    .flatten()
            })
            .unwrap_or(true)
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.db()
            .ok()
            .and_then(|c| archive_dir_setting(&c))
            .unwrap_or_else(|| self.default_archive_dir.clone())
    }

    /// Finish or undo archive moves interrupted by a crash or power loss.
    pub fn recover_archive(&self) {
        let cfg = cd_core::archive::ArchiveConfig {
            root: self.archive_dir(),
            force_copy: false,
        };
        if let Ok(conn) = self.db() {
            match cd_core::archive::recover(&conn, &cfg) {
                Ok(r) if r.finished > 0 || !r.failed.is_empty() => {
                    tracing::info!(?r, "recovered archive moves")
                }
                Ok(_) => {}
                Err(e) => tracing::error!("archive recovery failed: {e}"),
            }
        }
    }

    /// Run a closure on a fresh connection in the background, for work that
    /// must not delay the command that triggered it (reranking).
    pub fn spawn_background(&self, name: &'static str, f: impl FnOnce(&Connection) + Send + 'static) {
        let path = self.db_path.clone();
        std::thread::spawn(move || match cd_core::db::open_existing(&path) {
            Ok(conn) => f(&conn),
            Err(e) => tracing::error!("{name}: could not open database: {e}"),
        });
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
        if let Some(p) = self.player.lock().unwrap().take() {
            p.stop();
        }
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
