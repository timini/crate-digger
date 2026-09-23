//! Running the app's own slskd: started with the app, stopped on Quit.
//!
//! Everything is passed through environment variables so the Soulseek
//! password is never written to disk or shown in a process list. The web
//! API listens on loopback only, on a free port, with a fresh random API
//! key per start; the web interface is off and its login is randomised.
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::install;
use super::Slskd;
use crate::http::Transport;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum State {
    NotInstalled,
    Stopped,
    Starting,
    /// Running and signed in to Soulseek.
    Running,
    /// Running but not signed in (wrong login, or the network is down).
    SignedOut(String),
    Failed(String),
}

struct Live {
    child: Child,
    port: u16,
    api_key: String,
}

pub struct Managed {
    root: PathBuf,
    transport: Arc<dyn Transport>,
    live: Mutex<Option<Live>>,
    state: Mutex<State>,
}

pub struct Login {
    pub username: String,
    pub password: String,
}

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Stop an slskd left running by a previous session that crashed. The pid
/// is only signalled if that process is still slskd.
fn stop_orphan(pid_file: &Path) {
    let Some(pid) = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return;
    };
    #[cfg(unix)]
    {
        let name = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if name.contains("slskd") {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
    #[cfg(windows)]
    {
        let list = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if list.to_ascii_lowercase().contains("slskd") {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status();
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

impl Managed {
    pub fn new(root: PathBuf, transport: Arc<dyn Transport>) -> Self {
        let state = if install::installed(&root) {
            State::Stopped
        } else {
            State::NotInstalled
        };
        Self {
            root,
            transport,
            live: Mutex::new(None),
            state: Mutex::new(state),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn downloads_dir(staging: &Path) -> PathBuf {
        staging.join("soulseek")
    }

    pub fn state(&self) -> State {
        let mut live = self.live.lock().unwrap();
        // Notice a process that exited on its own.
        if let Some(l) = live.as_mut() {
            if let Ok(Some(status)) = l.child.try_wait() {
                *live = None;
                *self.state.lock().unwrap() = State::Failed(format!(
                    "slskd stopped unexpectedly ({status}). See {}.",
                    self.log().display()
                ));
            }
        }
        self.state.lock().unwrap().clone()
    }

    fn set(&self, s: State) {
        *self.state.lock().unwrap() = s;
    }

    pub fn log(&self) -> PathBuf {
        self.root.join("slskd.log")
    }

    /// The API of the running instance, if any.
    pub fn api(&self, downloads_dir: PathBuf) -> Option<Slskd> {
        self.live.lock().unwrap().as_ref().map(|l| Slskd {
            endpoint: format!("http://127.0.0.1:{}", l.port),
            api_key: l.api_key.clone(),
            transport: self.transport.clone(),
            downloads_dir,
            search_timeout: Duration::from_secs(15),
            poll: Duration::from_secs(1),
        })
    }

    /// Start slskd and wait until it answers and has tried to sign in.
    pub fn start(&self, login: &Login, staging: &Path) -> Result<(), String> {
        self.stop();
        if !install::installed(&self.root) {
            self.set(State::NotInstalled);
            return Err("slskd is not installed yet.".into());
        }
        self.set(State::Starting);
        let result = self.spawn(login, staging).and_then(|_| self.wait_ready(staging));
        if let Err(e) = &result {
            self.stop();
            self.set(State::Failed(e.clone()));
        }
        result
    }

    fn spawn(&self, login: &Login, staging: &Path) -> Result<(), String> {
        let app_dir = self.root.join("app");
        let downloads = Self::downloads_dir(staging);
        let incomplete = staging.join("soulseek-incomplete");
        for d in [&app_dir, &downloads, &incomplete] {
            std::fs::create_dir_all(d).map_err(|e| format!("Cannot create {}: {e}", d.display()))?;
        }
        let pid_file = self.root.join("slskd.pid");
        stop_orphan(&pid_file);
        let port = free_port().map_err(|e| format!("No free local port for slskd: {e}"))?;
        let api_key = secret();
        let log = std::fs::File::create(self.log()).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(install::binary(&self.root));
        cmd.arg("--app-dir")
            .arg(&app_dir)
            .env("SLSKD_HTTP_IP_ADDRESS", "127.0.0.1")
            .env("SLSKD_HTTP_PORT", port.to_string())
            .env("SLSKD_NO_HTTPS", "true")
            .env("SLSKD_HEADLESS", "true")
            .env("SLSKD_REMOTE_CONFIGURATION", "false")
            .env("SLSKD_NO_VERSION_CHECK", "true")
            .env("SLSKD_NO_LOGO", "true")
            .env("SLSKD_NO_SHARE_SCAN", "true")
            .env("SLSKD_API_KEY", format!("cidr=127.0.0.1/32,::1/128;{api_key}"))
            .env("SLSKD_USERNAME", secret())
            .env("SLSKD_PASSWORD", secret())
            .env("SLSKD_JWT_KEY", secret())
            .env("SLSKD_SLSK_USERNAME", &login.username)
            .env("SLSKD_SLSK_PASSWORD", &login.password)
            .env("SLSKD_DOWNLOADS_DIR", &downloads)
            .env("SLSKD_INCOMPLETE_DIR", &incomplete)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window.
            cmd.creation_flags(0x0800_0000);
        }
        let child = cmd.spawn().map_err(|e| format!("Could not start slskd: {e}"))?;
        let _ = std::fs::write(&pid_file, child.id().to_string());
        *self.live.lock().unwrap() = Some(Live { child, port, api_key });
        Ok(())
    }

    fn wait_ready(&self, staging: &Path) -> Result<(), String> {
        let api = self
            .api(Self::downloads_dir(staging))
            .ok_or("slskd did not start.")?;
        let started = Instant::now();
        loop {
            if let State::Failed(e) = self.state() {
                return Err(e);
            }
            match api.logged_in() {
                Ok(true) => {
                    self.set(State::Running);
                    return Ok(());
                }
                // Answering but not yet signed in: give the login some time.
                Ok(false) if started.elapsed() > Duration::from_secs(30) => {
                    self.set(State::SignedOut(
                        "slskd is running but not signed in to Soulseek. Check the username and password."
                            .into(),
                    ));
                    return Ok(());
                }
                _ if started.elapsed() > Duration::from_secs(90) => {
                    return Err(format!(
                        "slskd did not answer within 90 seconds. See {}.",
                        self.log().display()
                    ));
                }
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }

    pub fn stop(&self) {
        if let Some(mut l) = self.live.lock().unwrap().take() {
            let _ = l.child.kill();
            let _ = l.child.wait();
        }
        let _ = std::fs::remove_file(self.root.join("slskd.pid"));
        let installed = install::installed(&self.root);
        self.set(if installed {
            State::Stopped
        } else {
            State::NotInstalled
        });
    }

    /// Recheck the sign-in state of a running instance.
    pub fn refresh(&self, staging: &Path) -> State {
        if let (State::Running | State::SignedOut(_), Some(api)) =
            (self.state(), self.api(Self::downloads_dir(staging)))
        {
            match api.logged_in() {
                Ok(true) => self.set(State::Running),
                Ok(false) => self.set(State::SignedOut(
                    "slskd is not signed in to Soulseek right now.".into(),
                )),
                Err(_) => {}
            }
        }
        self.state()
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        self.stop();
    }
}
