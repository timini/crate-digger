//! Getting audio for identified candidates: acquire into staging, validate,
//! analyse, then mark ready.
//!
//! Each step is its own durable job with an idempotency key, so a restart
//! resumes where it stopped. Failed downloads and bad files are operational
//! failures recorded on the candidate; they never count as a dislike.

pub mod matching;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{Acquirer, AcquisitionQuery, SearchResult, TransferStatus};
use crate::domain::{CandidateStatus, Stage};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::library::{self, AudioProbe};
use crate::{meta, pipeline, Error, Result};

#[derive(Serialize, Deserialize)]
struct CandidatePayload {
    candidate_id: String,
    /// A result the user chose, which skips the search.
    #[serde(default)]
    chosen: Option<SearchResult>,
}

#[derive(Serialize, Deserialize)]
struct FilePayload {
    candidate_id: String,
    file_id: String,
    expected_duration_ms: Option<i64>,
}

#[derive(Default, Serialize, Deserialize)]
struct AcquireCheckpoint {
    /// The transfer in progress, if one was started.
    transfer_id: Option<String>,
    #[serde(default)]
    result_id: Option<String>,
    expected_duration_ms: Option<i64>,
    /// Results whose download failed; not tried again.
    #[serde(default)]
    failed: Vec<String>,
}

/// Default for `AcquireHandler::queued_watch`.
pub const QUEUED_WATCH: Duration = Duration::from_secs(120);
const QUEUED_RECHECK_MS: i64 = 5 * 60_000;

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

/// Park the candidate until the user picks a download. The job ends; the
/// user's choice queues a new one.
fn ask_user(
    ctx: &JobCtx<'_>,
    candidate_id: &str,
    outcome: &matching::Outcome,
    why: &str,
) -> std::result::Result<(), JobError> {
    ctx.conn
        .execute(
            "INSERT INTO acquisition_choice (candidate_id, connector, outcome, why, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(candidate_id) DO UPDATE SET connector = ?2, outcome = ?3, why = ?4,
                 created_at = ?5, resolved_at = NULL",
            params![
                candidate_id,
                ctx.job.connector.clone().unwrap_or_default(),
                serde_json::to_string(outcome).map_err(Error::from)?,
                why,
                ctx.now()
            ],
        )
        .map_err(Error::from)?;
    pipeline::set_status(
        ctx.conn,
        candidate_id,
        CandidateStatus::Blocked,
        Some(&format!("Choose a download: {why}")),
        ctx.now(),
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Choice {
    pub candidate_id: String,
    pub track_id: String,
    pub meta: crate::meta::TrackMeta,
    pub why: String,
    pub outcome: matching::Outcome,
    pub created_at: i64,
}

/// Downloads waiting for the user's choice, oldest first.
pub fn choices(conn: &Connection) -> Result<Vec<Choice>> {
    let mut stmt = conn.prepare(
        "SELECT a.candidate_id, c.track_id, a.why, a.outcome, a.created_at
         FROM acquisition_choice a JOIN candidate c ON c.id = a.candidate_id
         WHERE a.resolved_at IS NULL ORDER BY a.created_at",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(candidate_id, track_id, why, outcome, created_at)| {
            Ok(Choice {
                meta: meta::effective(conn, &track_id)?,
                outcome: serde_json::from_str(&outcome)?,
                candidate_id,
                track_id,
                why,
                created_at,
            })
        })
        .collect()
}

fn open_choice(conn: &Connection, candidate_id: &str) -> Result<(String, matching::Outcome)> {
    let (connector, outcome): (String, String) = conn
        .query_row(
            "SELECT connector, outcome FROM acquisition_choice WHERE candidate_id = ?1 AND resolved_at IS NULL",
            params![candidate_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound("download choice".into()))?;
    Ok((connector, serde_json::from_str(&outcome)?))
}

/// The user picked a result: download it.
pub fn choose(conn: &Connection, candidate_id: &str, result_id: &str, now: i64) -> Result<String> {
    let (connector, outcome) = open_choice(conn, candidate_id)?;
    let chosen = outcome
        .ranked
        .into_iter()
        .find(|a| a.result.result_id == result_id)
        .ok_or_else(|| Error::NotFound("that result".into()))?
        .result;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE acquisition_choice SET resolved_at = ?2 WHERE candidate_id = ?1",
        params![candidate_id, now],
    )?;
    pipeline::set_status(&tx, candidate_id, CandidateStatus::Active, None, now)?;
    let job = crate::jobs::enqueue(
        &tx,
        &NewJob::new(
            kinds::ACQUIRE,
            format!("acquire:{candidate_id}:{}", crate::util::new_id()),
            serde_json::to_value(CandidatePayload {
                candidate_id: candidate_id.to_string(),
                chosen: Some(chosen),
            })?,
        )
        .connector(&connector),
        now,
    )?;
    tx.commit()?;
    Ok(job.id)
}

/// None of the results will do. The candidate fails without counting as a dislike.
pub fn decline(conn: &Connection, candidate_id: &str, now: i64) -> Result<()> {
    open_choice(conn, candidate_id)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE acquisition_choice SET resolved_at = ?2 WHERE candidate_id = ?1",
        params![candidate_id, now],
    )?;
    pipeline::set_status(
        &tx,
        candidate_id,
        CandidateStatus::Failed,
        Some("No suitable download. This does not affect your ratings."),
        now,
    )?;
    tx.commit()?;
    Ok(())
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
    /// How long one run watches a queued transfer before handing the worker
    /// back and checking again later.
    pub queued_watch: Duration,
}

impl Handler for AcquireHandler {
    fn kind(&self) -> &'static str {
        kinds::ACQUIRE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let CandidatePayload { candidate_id, chosen } = ctx.payload()?;
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
        let mut checkpoint: AcquireCheckpoint = ctx.checkpoint().unwrap_or_default();

        let transfer_id = match checkpoint.transfer_id.clone() {
            Some(id) => id,
            None => {
                let best = match chosen.filter(|c| !checkpoint.failed.contains(&c.result_id)) {
                    Some(c) => c,
                    None => {
                        let m = meta::effective(ctx.conn, &track_id)?;
                        let query = AcquisitionQuery {
                            artist: m.artist.unwrap_or_default(),
                            title: m.title.unwrap_or_default(),
                            mix: m.mix,
                        };
                        let results: Vec<SearchResult> = acquirer
                            .search(&query)
                            .map_err(|e| ctx.adapter_error(e))?
                            .into_iter()
                            .filter(|r| !checkpoint.failed.contains(&r.result_id))
                            .collect();
                        if acquirer.exact_results() {
                            let Some(first) = results.into_iter().next() else {
                                return Err(fail_candidate(
                                    ctx,
                                    &candidate_id,
                                    "No downloadable copy was found.",
                                ));
                            };
                            first
                        } else {
                            let outcome = matching::assess(&query, &results);
                            let unattended = crate::settings::get_or(
                                ctx.conn,
                                crate::settings::keys::UNATTENDED_DOWNLOADS,
                                false,
                            )?;
                            match &outcome.decision {
                                matching::Decision::Nothing { why } => {
                                    return Err(fail_candidate(
                                        ctx,
                                        &candidate_id,
                                        &format!("No downloadable copy was found. {why}"),
                                    ));
                                }
                                matching::Decision::Auto if unattended => outcome.ranked[0].result.clone(),
                                matching::Decision::Auto => {
                                    let why = "Unattended downloads are off. The recommended copy meets the automatic rule.";
                                    let mut outcome = outcome.clone();
                                    outcome.decision = matching::Decision::Choose {
                                        recommended: Some(0),
                                        why: why.into(),
                                    };
                                    return ask_user(ctx, &candidate_id, &outcome, why);
                                }
                                matching::Decision::Choose { why, .. } => {
                                    return ask_user(ctx, &candidate_id, &outcome, why);
                                }
                            }
                        }
                    }
                };
                let dest = self.staging_root.join(&candidate_id);
                let key = format!("{}:{}", ctx.job.idempotency_key, checkpoint.failed.len());
                let id = acquirer
                    .enqueue(&best, &key, &dest)
                    .map_err(|e| ctx.adapter_error(e))?;
                checkpoint.transfer_id = Some(id.clone());
                checkpoint.result_id = Some(best.result_id.clone());
                checkpoint.expected_duration_ms = best.duration_ms.map(|d| d as i64);
                ctx.save_checkpoint(&checkpoint)?;
                pipeline::transition(ctx.conn, &candidate_id, Stage::Downloading, ctx.now())?;
                id
            }
        };

        let watching_since = std::time::Instant::now();
        let path = loop {
            match acquirer.status(&transfer_id).map_err(|e| ctx.adapter_error(e))? {
                TransferStatus::Completed { path } => break path,
                TransferStatus::InProgress { .. } => {
                    ctx.heartbeat()?;
                    std::thread::sleep(self.poll);
                }
                TransferStatus::Queued if watching_since.elapsed() < self.queued_watch => {
                    ctx.heartbeat()?;
                    std::thread::sleep(self.poll);
                }
                TransferStatus::Queued => {
                    return Err(JobError::Wait {
                        reason: "Waiting in the sharer's upload queue.".into(),
                        delay_ms: QUEUED_RECHECK_MS,
                    });
                }
                TransferStatus::Failed { reason } => {
                    // Try another copy next time rather than the same source.
                    checkpoint.failed.extend(checkpoint.result_id.take());
                    checkpoint.transfer_id = None;
                    ctx.save_checkpoint(&checkpoint)?;
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

#[cfg(test)]
mod tests;
