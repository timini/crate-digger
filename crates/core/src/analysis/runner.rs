//! Runs the analysis worker as a separate, low-priority process.
//!
//! Decoding untrusted files and running models happen outside the app, so a
//! crash, hang or runaway memory use ends one job with a reason instead of
//! taking playback or the UI with it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::protocol::{ErrorKind, Message, Request};

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// Longest a single job may take.
    pub timeout: Duration,
    /// Longest the worker may go without a heartbeat.
    pub heartbeat_timeout: Duration,
    /// Peak resident memory allowed, in KiB.
    pub memory_limit_kb: u64,
    /// Extra environment, used by tests to inject faults.
    pub env: Vec<(String, String)>,
}

impl RunnerConfig {
    pub fn new(exe: PathBuf) -> Self {
        RunnerConfig {
            exe,
            args: Vec::new(),
            timeout: Duration::from_secs(15 * 60),
            heartbeat_timeout: Duration::from_secs(30),
            memory_limit_kb: 2 * 1024 * 1024,
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WorkerError {
    #[error("could not start the analysis worker: {0}")]
    Spawn(String),
    #[error("the analysis worker stopped unexpectedly (exit {code:?}): {stderr}")]
    Crashed { code: Option<i32>, stderr: String },
    #[error("analysis took longer than {0} seconds")]
    TimedOut(u64),
    #[error("the analysis worker stopped responding")]
    Unresponsive,
    #[error("analysis used more than {limit_mb} MB of memory")]
    OverMemory { limit_mb: u64 },
    #[error("the analysis worker sent something unreadable: {0}")]
    BadOutput(String),
    #[error("{message}")]
    Failed { kind: ErrorKind, message: String },
}

impl WorkerError {
    /// Whether trying again could help. A file that cannot be decoded will
    /// not decode next time either.
    pub fn retryable(&self) -> bool {
        match self {
            WorkerError::Failed { kind, .. } => matches!(kind, ErrorKind::Internal | ErrorKind::Unreadable),
            WorkerError::Spawn(_) | WorkerError::BadOutput(_) => false,
            _ => true,
        }
    }
}

fn configure(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: nice() is async-signal-safe and touches only this process.
        unsafe {
            cmd.pre_exec(|| {
                libc::nice(10);
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

enum Line {
    Message(Message),
    Garbage(String),
}

/// Send one request and wait for its final message.
pub fn run(cfg: &RunnerConfig, request: &Request) -> Result<Message, WorkerError> {
    let mut cmd = Command::new(&cfg.exe);
    cmd.args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    configure(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| WorkerError::Spawn(format!("{}: {e}", cfg.exe.display())))?;

    let line = serde_json::to_string(request).map_err(|e| WorkerError::Spawn(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A worker that dies before reading shows up below as a crash.
        let _ = writeln!(stdin, "{line}");
    }

    let (tx, rx) = mpsc::channel::<Line>();
    let stdout = child.stdout.take().expect("piped stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let msg = match serde_json::from_str::<Message>(&line) {
                Ok(m) => Line::Message(m),
                Err(_) => Line::Garbage(line.chars().take(200).collect()),
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_handle = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).take(64 * 1024).read_to_string(&mut s);
        s
    });
    let stderr_tail = |h: std::thread::JoinHandle<String>| -> String {
        let s = h.join().unwrap_or_default();
        let t = s.trim();
        t[t.len().saturating_sub(500)..].to_string()
    };

    let started = Instant::now();
    let mut last_beat = Instant::now();
    loop {
        let remaining = cfg.timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            kill(&mut child);
            return Err(WorkerError::TimedOut(cfg.timeout.as_secs()));
        }
        let wait = remaining.min(cfg.heartbeat_timeout.saturating_sub(last_beat.elapsed()));
        match rx.recv_timeout(wait.max(Duration::from_millis(10))) {
            Ok(Line::Message(Message::Heartbeat { peak_rss_kb, .. })) => {
                last_beat = Instant::now();
                if peak_rss_kb > cfg.memory_limit_kb {
                    kill(&mut child);
                    return Err(WorkerError::OverMemory {
                        limit_mb: cfg.memory_limit_kb / 1024,
                    });
                }
            }
            Ok(Line::Message(Message::Error { kind, message })) => {
                let _ = child.wait();
                return Err(WorkerError::Failed { kind, message });
            }
            Ok(Line::Message(final_message)) => {
                let _ = child.wait();
                return Ok(final_message);
            }
            Ok(Line::Garbage(text)) => {
                kill(&mut child);
                return Err(WorkerError::BadOutput(text));
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_beat.elapsed() >= cfg.heartbeat_timeout {
                    kill(&mut child);
                    return Err(WorkerError::Unresponsive);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Output closed without a result: the worker died.
                let status = child.wait().ok();
                return Err(WorkerError::Crashed {
                    code: status.and_then(|s| s.code()),
                    stderr: stderr_tail(stderr_handle),
                });
            }
        }
    }
}
