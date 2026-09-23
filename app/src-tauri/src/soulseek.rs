//! The app's Soulseek connection: its own slskd, or one the user runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use cd_connectors::config::Connections;
use cd_connectors::credentials::{Credential, SecretStore};
use cd_connectors::http::Http;
use cd_connectors::slskd::install;
use cd_connectors::slskd::process::{Login, Managed, State};
use cd_connectors::slskd::{Locate, Slskd, CONNECTOR};
use cd_core::adapters::{AdapterError, AdapterResult};
use serde::Serialize;

pub struct Soulseek {
    managed: Managed,
    connections: Arc<RwLock<Connections>>,
    secrets: Arc<dyn SecretStore>,
    staging: Arc<dyn Fn() -> PathBuf + Send + Sync>,
    db_path: PathBuf,
    progress: AtomicU64,
    busy: AtomicBool,
}

#[derive(Serialize)]
pub struct Status {
    pub external: bool,
    pub state: State,
    pub installed: bool,
    pub busy: bool,
    pub downloaded_bytes: u64,
    pub download_size: Option<u64>,
    pub version: &'static str,
    pub licence: &'static str,
    pub source: &'static str,
}

impl Soulseek {
    pub fn new(
        root: PathBuf,
        connections: Arc<RwLock<Connections>>,
        secrets: Arc<dyn SecretStore>,
        staging: Arc<dyn Fn() -> PathBuf + Send + Sync>,
        db_path: PathBuf,
    ) -> Self {
        Self {
            managed: Managed::new(root, Arc::new(Http::default())),
            connections,
            secrets,
            staging,
            db_path,
            progress: AtomicU64::new(0),
            busy: AtomicBool::new(false),
        }
    }

    fn external(&self) -> bool {
        self.connections.read().unwrap().external_slskd
    }

    pub fn status(&self) -> Status {
        Status {
            external: self.external(),
            state: self.managed.state(),
            installed: install::installed(self.managed.root()),
            busy: self.busy.load(Ordering::Relaxed),
            downloaded_bytes: self.progress.load(Ordering::Relaxed),
            download_size: install::release().map(|r| r.size),
            version: install::VERSION,
            licence: install::LICENCE,
            source: install::SOURCE,
        }
    }

    fn login(&self) -> Result<Login, String> {
        let get = |c| self.secrets.get(c).map_err(|e| e.to_string());
        match (
            get(Credential::SoulseekUsername)?,
            get(Credential::SoulseekPassword)?,
        ) {
            (Some(username), Some(password)) => Ok(Login { username, password }),
            _ => Err("Save your Soulseek username and password first.".into()),
        }
    }

    /// Let paused downloads try again once Soulseek works.
    fn resume_downloads(&self) {
        if let Ok(conn) = cd_core::db::open(&self.db_path) {
            let _ = cd_core::jobs::set_connector_status(
                &conn,
                CONNECTOR,
                cd_core::jobs::ConnectorStatus::Ok,
                None,
                cd_core::util::now_ms(),
            );
        }
    }

    /// Start the managed slskd if it is installed and the login is saved.
    pub fn start_if_ready(&self) {
        if self.external() || !install::installed(self.managed.root()) {
            return;
        }
        if let Ok(login) = self.login() {
            // Failures are recorded in the state that Settings shows.
            let _ = self.run_start(&login);
        }
    }

    fn run_start(&self, login: &Login) -> Result<(), String> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("Soulseek is already starting.".into());
        }
        let result = self.managed.start(login, &(self.staging)());
        self.busy.store(false, Ordering::SeqCst);
        if result.is_ok() && self.managed.state() == State::Running {
            self.resume_downloads();
        }
        if let Err(e) = &result {
            tracing::warn!("slskd did not start: {e}");
        }
        result
    }

    /// Download slskd if needed, then start it. Slow; run off the UI thread.
    pub fn setup(&self) -> Result<(), String> {
        let login = self.login()?;
        if !install::installed(self.managed.root()) {
            if self.busy.swap(true, Ordering::SeqCst) {
                return Err("slskd is already being set up.".into());
            }
            self.progress.store(0, Ordering::Relaxed);
            let result = install::download(self.managed.root(), &self.progress);
            self.busy.store(false, Ordering::SeqCst);
            result?;
        }
        self.run_start(&login)
    }

    pub fn stop(&self) {
        self.managed.stop();
    }
}

impl Locate for Soulseek {
    fn locate(&self) -> AdapterResult<Slskd> {
        let config = self.connections.read().unwrap().clone();
        if config.external_slskd {
            let key = self
                .secrets
                .get(Credential::Slskd)?
                .ok_or_else(|| AdapterError::Auth("Save the API key of your slskd in Settings.".into()))?;
            if config.slskd_downloads_dir.trim().is_empty() {
                return Err(AdapterError::Auth(
                    "Enter your slskd's downloads folder in Settings so finished files can be found.".into(),
                ));
            }
            return Ok(Slskd {
                endpoint: config.slskd_endpoint,
                api_key: key,
                transport: Arc::new(Http::default()),
                downloads_dir: PathBuf::from(config.slskd_downloads_dir),
                search_timeout: std::time::Duration::from_secs(15),
                poll: std::time::Duration::from_secs(1),
            });
        }
        match self.managed.state() {
            State::Running => {}
            State::SignedOut(why) => return Err(AdapterError::Auth(why)),
            _ => {
                return Err(AdapterError::Auth(
                    "Soulseek is not running. Set it up in Settings.".into(),
                ))
            }
        }
        self.managed
            .api(Managed::downloads_dir(&(self.staging)()))
            .ok_or_else(|| AdapterError::Auth("Soulseek is not running. Set it up in Settings.".into()))
    }
}
