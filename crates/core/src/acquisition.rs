//! Getting audio for identified candidates: acquire into staging, validate,
//! analyse, then mark ready.
//!
//! Each step is its own durable job with an idempotency key, so a restart
//! resumes where it stopped. Failed downloads and bad files are operational
//! failures recorded on the candidate; they never count as a dislike.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::adapters::{Acquirer, AcquisitionQuery, TransferStatus};
use crate::domain::{CandidateStatus, Stage};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::library::{self, AudioProbe};
use crate::{meta, pipeline, Error, Result};

#[derive(Serialize, Deserialize)]
struct CandidatePayload {
    candidate_id: String,
}

#[derive(Serialize, Deserialize)]
struct FilePayload {
    candidate_id: String,
    file_id: String,
    expected_duration_ms: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct AcquireCheckpoint {
    transfer_id: String,
    expected_duration_ms: Option<i64>,
}

fn candidate_track(conn: &Connection, candidate_id: &str) -> Result<String> {
    Ok(conn.query_row(
        "SELECT track_id FROM candidate WHERE id = ?1",
        params![candidate_id],
        |r| r.get(0),
    )?)
}

/// Record a failure on the candidate and stop the job. Never touches
/// ratings.
fn fail_candidate(ctx: &JobCtx<'_>, candidate_id: &str, reason: &str) -> JobError {
    if let Err(e) = pipeline::set_status(
        ctx.conn,
        candidate_id,
        CandidateStatus::Failed,
        Some(reason),
        ctx.now(),
    ) {
        tracing::warn!("could not record candidate failure: {e}");
    }
    JobError::Fatal(reason.to_string())
}

fn on_last_attempt(ctx: &JobCtx<'_>) -> bool {
    ctx.job.attempts >= ctx.job.max_attempts
}

pub struct AcquireHandler {
    /// Chosen by the job's connector; the first is the default.
    pub acquirers: Vec<Arc<dyn Acquirer>>,
    pub staging_root: PathBuf,
    pub probe: Arc<dyn AudioProbe>,
    /// How often to check an in-progress transfer.
    pub poll: Duration,
}

impl Handler for AcquireHandler {
    fn kind(&self) -> &'static str {
        kinds::ACQUIRE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let CandidatePayload { candidate_id } = ctx.payload()?;
        let acquirer = match &ctx.job.connector {
            None => self.acquirers.first(),
            Some(c) => self.acquirers.iter().find(|a| a.id() == c),
        }
        .ok_or_else(|| JobError::Auth {
            connector: ctx.job.connector.clone().unwrap_or_default(),
            message: "No download source is set up yet. Connect Soulseek in Settings.".into(),
        })?
        .clone();
        let track_id = candidate_track(ctx.conn, &candidate_id)?;

        let checkpoint = match ctx.checkpoint::<AcquireCheckpoint>() {
            Some(c) => c,
            None => {
                let m = meta::effective(ctx.conn, &track_id)?;
                let query = AcquisitionQuery {
                    artist: m.artist.unwrap_or_default(),
                    title: m.title.unwrap_or_default(),
                    mix: m.mix,
                };
                let results = acquirer.search(&query).map_err(|e| ctx.adapter_error(e))?;
                // Candidate selection and the auto-match rule arrive with the
                // slskd integration (#12); the demo source returns one result.
                let Some(best) = results.into_iter().next() else {
                    return Err(fail_candidate(
                        ctx,
                        &candidate_id,
                        "No downloadable copy was found.",
                    ));
                };
                let dest = self.staging_root.join(&candidate_id);
                let transfer_id = acquirer
                    .enqueue(&best, &ctx.job.idempotency_key, &dest)
                    .map_err(|e| ctx.adapter_error(e))?;
                let c = AcquireCheckpoint {
                    transfer_id,
                    expected_duration_ms: best.duration_ms.map(|d| d as i64),
                };
                ctx.save_checkpoint(&c)?;
                pipeline::transition(ctx.conn, &candidate_id, Stage::Downloading, ctx.now())?;
                c
            }
        };

        let path = loop {
            match acquirer
                .status(&checkpoint.transfer_id)
                .map_err(|e| ctx.adapter_error(e))?
            {
                TransferStatus::Completed { path } => break path,
                TransferStatus::Queued | TransferStatus::InProgress { .. } => {
                    ctx.heartbeat()?;
                    std::thread::sleep(self.poll);
                }
                TransferStatus::Failed { reason } => {
                    let msg = format!("Download failed: {reason}");
                    return Err(if on_last_attempt(ctx) {
                        fail_candidate(ctx, &candidate_id, &msg)
                    } else {
                        JobError::Retryable(msg)
                    });
                }
                TransferStatus::Cancelled => {
                    return Err(fail_candidate(ctx, &candidate_id, "The download was cancelled."));
                }
            }
        };

        let registered = library::register_staged(ctx.conn, &path, &track_id, self.probe.as_ref())?;
        pipeline::transition(ctx.conn, &candidate_id, Stage::Validating, ctx.now())?;
        crate::jobs::enqueue(
            ctx.conn,
            &NewJob::new(
                kinds::VALIDATE,
                format!("validate:{}", registered.file_id),
                serde_json::to_value(FilePayload {
                    candidate_id,
                    file_id: registered.file_id,
                    expected_duration_ms: checkpoint.expected_duration_ms,
                })
                .map_err(Error::from)?,
            ),
            ctx.now(),
        )?;
        Ok(())
    }
}

/// Tolerance between the length a source advertised and the decoded length.
const DURATION_TOLERANCE_MS: i64 = 5_000;

pub struct ValidateHandler {
    pub probe: Arc<dyn AudioProbe>,
}

impl Handler for ValidateHandler {
    fn kind(&self) -> &'static str {
        kinds::VALIDATE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let payload: FilePayload = ctx.payload()?;
        let file = library::file(ctx.conn, &payload.file_id)?;
        let decoded = match self.probe.decode_full(std::path::Path::new(&file.path)) {
            Ok(ms) => ms,
            Err(e) => {
                library::mark_corrupt(ctx.conn, &file.id, &e)?;
                return Err(fail_candidate(
                    ctx,
                    &payload.candidate_id,
                    &format!(
                        "The downloaded file could not be decoded ({e}). It was not played and does not affect \
                         your ratings; retry to fetch another copy."
                    ),
                ));
            }
        };
        if let Some(expected) = payload.expected_duration_ms {
            if (decoded - expected).abs() > DURATION_TOLERANCE_MS {
                pipeline::set_status(
                    ctx.conn,
                    &payload.candidate_id,
                    CandidateStatus::Blocked,
                    Some(&format!(
                        "Needs review: the download is {} s long but the source listed {} s. It may be a \
                         different mix or an incomplete file.",
                        decoded / 1000,
                        expected / 1000
                    )),
                    ctx.now(),
                )?;
                return Ok(());
            }
        }
        ctx.conn
            .execute(
                "UPDATE audio_file SET duration_ms = ?2 WHERE id = ?1",
                params![file.id, decoded],
            )
            .map_err(Error::from)?;
        crate::analysis::handler::queue_for_candidate(ctx.conn, &payload.candidate_id, &file.id, ctx.now())?;
        Ok(())
    }
}
