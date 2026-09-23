//! The analysis job: run the worker on a file, store its features and, for
//! a candidate, move it to Ready.

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::protocol::{Analysis, ErrorKind, FingerprintOut, Message, ModelRef, Request};
use super::runner::{self, RunnerConfig, WorkerError};
use super::{store, FeatureVersion};
use crate::domain::{CandidateStatus, Stage};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError};
use crate::{library, pipeline, review, Error};

/// Produces analyses. The app uses [`ProcessAnalyzer`]; tests may use an
/// in-process fake.
pub trait Analyzer: Send + Sync {
    fn analyse(&self, path: &Path) -> Result<Analysis, WorkerError>;
    fn fingerprint(&self, path: &Path, speed: f64) -> Result<FingerprintOut, WorkerError>;
    /// The embedding version this analyzer produces.
    fn version(&self) -> FeatureVersion;
}

/// Runs each request in a separate worker process.
pub struct ProcessAnalyzer {
    pub config: RunnerConfig,
    /// The version whose embeddings drive ranking: the installed model's,
    /// or the built-in baseline's.
    pub version: FeatureVersion,
    /// Downloaded models to run alongside the baseline.
    pub models: Vec<ModelRef>,
}

impl Analyzer for ProcessAnalyzer {
    fn analyse(&self, path: &Path) -> Result<Analysis, WorkerError> {
        match runner::run(
            &self.config,
            &Request::Analyse {
                path: path.to_string_lossy().into_owned(),
                models: self.models.clone(),
            },
        )? {
            Message::Analysis(a) => Ok(*a),
            other => Err(WorkerError::BadOutput(format!(
                "expected an analysis, got {other:?}"
            ))),
        }
    }

    fn fingerprint(&self, path: &Path, speed: f64) -> Result<FingerprintOut, WorkerError> {
        match runner::run(
            &self.config,
            &Request::Fingerprint {
                path: path.to_string_lossy().into_owned(),
                speed,
            },
        )? {
            Message::Fingerprint(f) => Ok(f),
            other => Err(WorkerError::BadOutput(format!(
                "expected a fingerprint, got {other:?}"
            ))),
        }
    }

    fn version(&self) -> FeatureVersion {
        self.version.clone()
    }
}

/// An analyzer that can be replaced while jobs are running, for example
/// when the user switches models.
pub struct SwitchableAnalyzer {
    current: std::sync::RwLock<Arc<dyn Analyzer>>,
}

impl SwitchableAnalyzer {
    pub fn new(initial: Arc<dyn Analyzer>) -> Self {
        SwitchableAnalyzer {
            current: std::sync::RwLock::new(initial),
        }
    }

    pub fn set(&self, next: Arc<dyn Analyzer>) {
        *self.current.write().unwrap() = next;
    }

    fn get(&self) -> Arc<dyn Analyzer> {
        self.current.read().unwrap().clone()
    }
}

impl Analyzer for SwitchableAnalyzer {
    fn analyse(&self, path: &Path) -> Result<Analysis, WorkerError> {
        self.get().analyse(path)
    }

    fn fingerprint(&self, path: &Path, speed: f64) -> Result<FingerprintOut, WorkerError> {
        self.get().fingerprint(path, speed)
    }

    fn version(&self) -> FeatureVersion {
        self.get().version()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysePayload {
    pub file_id: String,
    pub candidate_id: Option<String>,
}

/// Called after features are stored, to match the track against the
/// library (see `identity::matching`). Errors are logged, not fatal.
pub type AfterAnalysis = Arc<dyn Fn(&Connection, &str, &str) -> crate::Result<()> + Send + Sync>;

pub struct AnalysisHandler {
    pub analyzer: Arc<dyn Analyzer>,
    pub after: Option<AfterAnalysis>,
}

fn finish_candidate(ctx: &JobCtx<'_>, candidate_id: &Option<String>) -> Result<(), JobError> {
    if let Some(c) = candidate_id {
        // Matching may have held the candidate back (already owned).
        if !crate::identity::matching::candidate_is_active(ctx.conn, c).unwrap_or(false) {
            return Ok(());
        }
        // Analysis serves ranking; a track with playable audio can be
        // reviewed even if its analysis failed.
        match review::mark_ready(ctx.conn, c, ctx.now()) {
            Ok(()) | Err(Error::NotFound(_)) => {}
            Err(e) => {
                pipeline::set_status(
                    ctx.conn,
                    c,
                    CandidateStatus::Failed,
                    Some(&e.to_string()),
                    ctx.now(),
                )?;
            }
        }
    }
    Ok(())
}

impl Handler for AnalysisHandler {
    fn kind(&self) -> &'static str {
        kinds::ANALYSE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> Result<(), JobError> {
        let p: AnalysePayload = ctx.payload()?;
        let file = match library::file(ctx.conn, &p.file_id) {
            Ok(f) => f,
            // The file was cleared from staging or merged away meanwhile.
            Err(Error::NotFound(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let version = self.analyzer.version();
        match self.analyzer.analyse(Path::new(&file.path)) {
            Ok(a) => {
                store::store(ctx.conn, &file.track_id, &file.id, &a)?;
                if let Some(after) = &self.after {
                    if let Err(e) = after(ctx.conn, &file.track_id, &file.id) {
                        tracing::warn!("identity matching after analysis failed: {e}");
                    }
                }
                finish_candidate(ctx, &p.candidate_id)
            }
            Err(e) if e.retryable() && ctx.job.attempts < ctx.job.max_attempts => {
                Err(JobError::Retryable(e.to_string()))
            }
            Err(e) => {
                let reason = e.to_string();
                match &e {
                    WorkerError::Failed {
                        kind: ErrorKind::NotFound,
                        ..
                    } => library::mark_missing(ctx.conn, &file.id, &file.path)?,
                    WorkerError::Failed {
                        kind: ErrorKind::Corrupt | ErrorKind::Unsupported,
                        message,
                    } => library::mark_corrupt(ctx.conn, &file.id, message)?,
                    _ => {}
                }
                store::set_state(ctx.conn, &file.track_id, &version, "failed", Some(&reason))?;
                finish_candidate(ctx, &p.candidate_id)?;
                Err(JobError::Fatal(format!("Analysis failed: {reason}")))
            }
        }
    }
}

/// The analyse step of the discovery pipeline for a candidate's file.
pub fn queue_for_candidate(
    conn: &Connection,
    candidate_id: &str,
    file_id: &str,
    now: i64,
) -> crate::Result<()> {
    pipeline::transition(conn, candidate_id, Stage::Analysing, now)?;
    crate::jobs::enqueue(
        conn,
        &crate::jobs::NewJob::new(
            kinds::ANALYSE,
            format!("analyse:{file_id}"),
            serde_json::to_value(AnalysePayload {
                file_id: file_id.to_string(),
                candidate_id: Some(candidate_id.to_string()),
            })?,
        ),
        now,
    )?;
    Ok(())
}
