//! Turning source proposals into candidates.
//!
//! Every candidate keeps its evidence: where it came from, when, and the
//! supporting excerpt. Only candidates backed by retrievable evidence are
//! marked verified, and only verified candidates may be acquired
//! automatically.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{
    CandidateProposal, DiscoveryInput, DiscoveryRequest, DiscoverySource, EvidenceProposal, Seed,
    LLM_EVIDENCE,
};
use crate::domain::{Field, SeedKind, Stage};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::util::new_id;
use crate::{meta, pipeline, Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub created: Vec<String>,
    /// Proposals matching a track already known, by artist, title and mix.
    pub already_known: usize,
    /// Created without independent evidence; never acquired automatically.
    pub unverified: usize,
    /// Existing unverified candidates that new evidence has now verified.
    pub newly_verified: Vec<String>,
    /// Existing candidates this run proposed again.
    pub matched: Vec<String>,
}

/// Does a track with this artist, title and mix exist already? Title
/// similarity is not identity, so this only catches exact (case-insensitive)
/// repeats of the same proposal.
fn known_track(conn: &Connection, p: &CandidateProposal) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT track_id FROM track_meta
             WHERE artist = ?1 COLLATE NOCASE AND title = ?2 COLLATE NOCASE
               AND COALESCE(mix, '') = COALESCE(?3, '') COLLATE NOCASE
             LIMIT 1",
            params![p.artist.trim(), p.title.trim(), p.mix.as_deref().map(str::trim)],
            |r| r.get(0),
        )
        .optional()?)
}

/// Evidence verifies a candidate only when it points at something the user
/// can check (a page, a catalogue entry, pasted text) and did not come from a
/// model alone.
fn verifies(e: &EvidenceProposal) -> bool {
    e.source_kind != LLM_EVIDENCE && (e.source_url.is_some() || e.supplied_text_id.is_some())
}

fn add_evidence(tx: &Connection, candidate_id: &str, p: &CandidateProposal, now: i64) -> Result<()> {
    for e in p
        .evidence
        .iter()
        .filter(|e| e.source_url.is_some() || e.supplied_text_id.is_some())
    {
        tx.execute(
            "INSERT INTO evidence (id, candidate_id, source_kind, source_url, supplied_text_id,
                                   retrieved_at, excerpt, confidence)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
             WHERE NOT EXISTS (
                 SELECT 1 FROM evidence WHERE candidate_id = ?2 AND source_kind = ?3
                   AND COALESCE(source_url, '') = COALESCE(?4, '')
                   AND COALESCE(supplied_text_id, '') = COALESCE(?5, '') AND excerpt = ?7)",
            params![
                new_id(),
                candidate_id,
                e.source_kind,
                e.source_url,
                e.supplied_text_id,
                now,
                e.excerpt.chars().take(500).collect::<String>(),
                e.confidence.clamp(0.0, 1.0)
            ],
        )?;
    }
    for reason in &p.reasons {
        tx.execute(
            "INSERT INTO explanation (id, candidate_id, reason)
             SELECT ?1, ?2, ?3 WHERE NOT EXISTS
                 (SELECT 1 FROM explanation WHERE candidate_id = ?2 AND reason = ?3)",
            params![new_id(), candidate_id, reason],
        )?;
    }
    Ok(())
}

pub fn ingest(
    conn: &Connection,
    source_id: &str,
    proposals: &[CandidateProposal],
    now: i64,
) -> Result<IngestSummary> {
    let mut summary = IngestSummary::default();
    let tx = conn.unchecked_transaction()?;
    for p in proposals {
        if p.artist.trim().is_empty() || p.title.trim().is_empty() {
            continue;
        }
        let verified = p.evidence.iter().any(verifies);
        let confidence = p
            .evidence
            .iter()
            .filter(|e| verifies(e))
            .map(|e| e.confidence)
            .fold(0.0f64, f64::max);
        if let Some(track_id) = known_track(&tx, p)? {
            summary.already_known += 1;
            // New evidence for an existing candidate is kept, and can verify it.
            let existing: Option<(String, bool)> = tx
                .query_row(
                    "SELECT id, verified FROM candidate WHERE track_id = ?1",
                    params![track_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((candidate_id, was_verified)) = existing {
                add_evidence(&tx, &candidate_id, p, now)?;
                summary.matched.push(candidate_id.clone());
                if verified && !was_verified {
                    tx.execute(
                        "UPDATE candidate SET verified = 1, confidence = MAX(COALESCE(confidence, 0), ?2),
                                updated_at = ?3 WHERE id = ?1",
                        params![candidate_id, confidence, now],
                    )?;
                    summary.newly_verified.push(candidate_id);
                }
            }
            continue;
        }
        let track_id = meta::create_track(&tx)?;
        meta::set_extracted(
            &tx,
            &track_id,
            source_id,
            &[
                (Field::Artist, Some(p.artist.trim().to_string())),
                (Field::Title, Some(p.title.trim().to_string())),
                (Field::Mix, p.mix.clone()),
                (Field::Label, p.label.clone()),
                (Field::Release, p.release.clone()),
            ],
        )?;
        let candidate_id = new_id();
        tx.execute(
            "INSERT INTO candidate (id, track_id, stage, verified, confidence, score, created_at, updated_at)
             VALUES (?1, ?2, 'candidate', ?3, ?4, ?4, ?5, ?5)",
            params![candidate_id, track_id, verified, confidence, now],
        )?;
        add_evidence(&tx, &candidate_id, p, now)?;
        if !verified {
            summary.unverified += 1;
        }
        summary.created.push(candidate_id);
    }
    tx.commit()?;
    Ok(summary)
}

/// Mark a candidate identified. In milestone 1 a proposal's explicit
/// artist, title and mix are its identity; version matching arrives in #8.
pub fn identify(conn: &Connection, candidate_id: &str, now: i64) -> Result<()> {
    pipeline::transition(conn, candidate_id, Stage::Identified, now)
}

/// Queue audio acquisition. Unverified candidates are refused: they may
/// only be acquired after the user asks for them explicitly.
pub fn queue_acquisition(conn: &Connection, candidate_id: &str, connector: &str, now: i64) -> Result<String> {
    let verified: bool = conn
        .query_row(
            "SELECT verified FROM candidate WHERE id = ?1",
            params![candidate_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("candidate {candidate_id}")))?;
    if !verified {
        return Err(Error::Invalid(
            "this candidate has no supporting evidence and cannot be acquired automatically".into(),
        ));
    }
    pipeline::transition(conn, candidate_id, Stage::AcquisitionQueued, now)?;
    Ok(crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::ACQUIRE,
            format!("acquire:{candidate_id}"),
            serde_json::json!({ "candidate_id": candidate_id }),
        )
        .connector(connector),
        now,
    )?
    .id)
}

pub fn seeds(conn: &Connection) -> Result<Vec<Seed>> {
    let mut stmt = conn.prepare("SELECT kind, value FROM seed ORDER BY created_at")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Seed {
                kind: r.get::<_, SeedKind>(0)?,
                value: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Saved seeds plus the artists and labels of recent positively rated
/// tracks, so ratings widen what discovery looks at.
pub fn seeds_for_run(conn: &Connection) -> Result<Vec<Seed>> {
    let mut out = seeds(conn)?;
    let mut stmt = conn.prepare(
        "SELECT m.artist, m.label FROM effective_rating r JOIN track_meta m ON m.track_id = r.track_id
         WHERE r.kind IN ('star1', 'star2', 'star3')
         ORDER BY r.created_at DESC LIMIT 20",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (artist, label) in rows {
        for (kind, value) in [(SeedKind::Artist, artist), (SeedKind::Label, label)] {
            let Some(value) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
                continue;
            };
            if !out
                .iter()
                .any(|s| s.kind == kind && s.value.eq_ignore_ascii_case(&value))
            {
                out.push(Seed { kind, value });
            }
        }
    }
    Ok(out)
}

/// Store pasted text; evidence refers to it by the returned id.
pub fn save_supplied_text(conn: &Connection, label: Option<&str>, text: &str, now: i64) -> Result<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 200_000 {
        return Err(Error::Invalid(
            "Paste between 1 and 200,000 characters of text.".into(),
        ));
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO supplied_text (id, label, text, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, label.map(str::trim).filter(|l| !l.is_empty()), text, now],
    )?;
    Ok(id)
}

/// What a queued discovery job works from. Pasted text is referenced by id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveryJob {
    #[default]
    Seeds,
    Page {
        url: String,
    },
    Text {
        supplied_text_id: String,
    },
}

impl DiscoveryJob {
    fn describe(&self) -> String {
        match self {
            DiscoveryJob::Seeds => "seeds and ratings".into(),
            DiscoveryJob::Page { url } => format!("page {url}"),
            DiscoveryJob::Text { .. } => "pasted text".into(),
        }
    }

    fn resolve(&self, conn: &Connection) -> Result<DiscoveryInput> {
        Ok(match self {
            DiscoveryJob::Seeds => DiscoveryInput::Seeds,
            DiscoveryJob::Page { url } => DiscoveryInput::Page { url: url.clone() },
            DiscoveryJob::Text { supplied_text_id } => DiscoveryInput::Text {
                supplied_text_id: supplied_text_id.clone(),
                text: conn
                    .query_row(
                        "SELECT text FROM supplied_text WHERE id = ?1",
                        params![supplied_text_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| Error::NotFound("pasted text".into()))?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRun {
    pub source: String,
    pub input: String,
    pub started_at: i64,
    pub finished_at: i64,
    /// `found`, `empty` or `failed`.
    pub outcome: String,
    pub created: i64,
    pub already_known: i64,
    pub unverified: i64,
    pub detail: Option<String>,
    /// The playlist the run was for, if any.
    #[serde(default)]
    pub playlist_id: Option<String>,
}

fn record_run(conn: &Connection, run: &SourceRun) -> Result<()> {
    conn.execute(
        "INSERT INTO source_run (id, source, input, started_at, finished_at, outcome, created,
                                 already_known, unverified, detail, playlist_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            new_id(),
            run.source,
            run.input,
            run.started_at,
            run.finished_at,
            run.outcome,
            run.created,
            run.already_known,
            run.unverified,
            run.detail,
            run.playlist_id
        ],
    )?;
    Ok(())
}

pub fn recent_runs(conn: &Connection, limit: i64) -> Result<Vec<SourceRun>> {
    let mut stmt = conn.prepare(
        "SELECT source, input, started_at, finished_at, outcome, created, already_known, unverified, detail,
                playlist_id
         FROM source_run ORDER BY finished_at DESC, rowid DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(SourceRun {
                source: r.get(0)?,
                input: r.get(1)?,
                started_at: r.get(2)?,
                finished_at: r.get(3)?,
                outcome: r.get(4)?,
                created: r.get(5)?,
                already_known: r.get(6)?,
                unverified: r.get(7)?,
                detail: r.get(8)?,
                playlist_id: r.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// True when no seed-based run for `connector` has started within
/// `interval_ms` and none is waiting, so a periodic refresh should queue one.
pub fn refresh_due(conn: &Connection, connector: &str, interval_ms: i64, now: i64) -> Result<bool> {
    let pending: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM job WHERE kind = ?1 AND connector = ?2
                          AND state IN ('queued', 'running', 'blocked', 'paused'))",
        params![kinds::DISCOVER, connector],
        |r| r.get(0),
    )?;
    if pending {
        return Ok(false);
    }
    let last: Option<i64> = conn.query_row(
        "SELECT MAX(started_at) FROM source_run WHERE source = ?1 AND input = ?2 AND playlist_id IS NULL",
        params![connector, DiscoveryJob::Seeds.describe()],
        |r| r.get(0),
    )?;
    Ok(last.is_none_or(|t| now - t >= interval_ms))
}

#[derive(Serialize, Deserialize)]
struct DiscoverPayload {
    limit: usize,
    #[serde(default)]
    input: DiscoveryJob,
    /// A run for one playlist uses its brief and seeds, and its results
    /// are suggested for that playlist.
    #[serde(default)]
    playlist: Option<String>,
}

/// A discovery source and the acquirer its verified candidates go to.
pub struct Route {
    pub source: Arc<dyn DiscoverySource>,
    pub acquirer_id: String,
    /// Queue a YouTube reference lookup for each new candidate.
    pub video_lookup: bool,
}

/// Asks a source for candidates, stores them with their evidence and
/// queues acquisition for the verified ones. The job's connector names the
/// source.
pub struct DiscoverHandler {
    pub routes: Vec<Route>,
}

impl DiscoverHandler {
    pub fn new(source: Arc<dyn DiscoverySource>, acquirer_id: &str) -> Self {
        Self { routes: vec![] }.route(source, acquirer_id)
    }

    pub fn route(mut self, source: Arc<dyn DiscoverySource>, acquirer_id: &str) -> Self {
        self.routes.push(Route {
            source,
            acquirer_id: acquirer_id.to_string(),
            video_lookup: false,
        });
        self
    }

    /// Look up YouTube references for candidates from the last added route.
    pub fn with_video_lookup(mut self) -> Self {
        if let Some(r) = self.routes.last_mut() {
            r.video_lookup = true;
        }
        self
    }
}

impl Handler for DiscoverHandler {
    fn kind(&self) -> &'static str {
        kinds::DISCOVER
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let payload: DiscoverPayload = ctx.payload()?;
        let route = match &ctx.job.connector {
            None => self.routes.first(),
            Some(c) => self.routes.iter().find(|r| r.source.id() == c),
        }
        .ok_or_else(|| JobError::Fatal("This discovery source is not available.".into()))?;
        let started_at = ctx.now();
        // A playlist deleted since the run was queued has nothing to discover for.
        let playlist = match &payload.playlist {
            Some(p) => match crate::workspace::brief(ctx.conn, p)? {
                Some(brief) => Some((p.clone(), brief)),
                None => return Err(JobError::Fatal("The playlist no longer exists.".into())),
            },
            None => None,
        };
        let request = DiscoveryRequest {
            seeds: match &playlist {
                Some((p, _)) => crate::workspace::seeds_for_run(ctx.conn, p)?,
                None => seeds_for_run(ctx.conn)?,
            },
            limit: payload.limit,
            input: payload.input.resolve(ctx.conn)?,
            brief: playlist
                .as_ref()
                .map(|(_, b)| b.clone())
                .filter(|b| !b.is_empty()),
        };
        let mut run = SourceRun {
            source: route.source.id().to_string(),
            input: payload.input.describe(),
            started_at,
            finished_at: started_at,
            outcome: "failed".into(),
            created: 0,
            already_known: 0,
            unverified: 0,
            detail: None,
            playlist_id: playlist.as_ref().map(|(p, _)| p.clone()),
        };
        let proposals = match route.source.discover(&request) {
            Ok(p) => p,
            Err(e) => {
                run.finished_at = ctx.now();
                run.detail = Some(e.to_string());
                record_run(ctx.conn, &run)?;
                return Err(ctx.adapter_error(e));
            }
        };
        let now = ctx.now();
        let summary = ingest(ctx.conn, route.source.id(), &proposals, now)?;
        for id in &summary.created {
            identify(ctx.conn, id, now)?;
            if route.video_lookup {
                let track_id: String = ctx
                    .conn
                    .query_row("SELECT track_id FROM candidate WHERE id = ?1", params![id], |r| {
                        r.get(0)
                    })
                    .map_err(Error::from)?;
                crate::youtube::queue_lookup(ctx.conn, &track_id, false, now)?;
            }
            let verified: bool = ctx
                .conn
                .query_row("SELECT verified FROM candidate WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .map_err(Error::from)?;
            if verified {
                queue_acquisition(ctx.conn, id, &route.acquirer_id, now)?;
            }
        }
        for id in &summary.newly_verified {
            if pipeline::state(ctx.conn, id)?.stage == Stage::Identified {
                queue_acquisition(ctx.conn, id, &route.acquirer_id, now)?;
            }
        }
        if let Some((p, _)) = &playlist {
            for id in summary.created.iter().chain(&summary.matched) {
                crate::workspace::add_context(ctx.conn, id, p, now)?;
            }
        }
        run.finished_at = now;
        run.outcome = if summary.created.is_empty() {
            "empty"
        } else {
            "found"
        }
        .into();
        run.created = summary.created.len() as i64;
        run.already_known = summary.already_known as i64;
        run.unverified = summary.unverified as i64;
        record_run(ctx.conn, &run)?;
        ctx.save_checkpoint(&summary)
    }
}

/// Queue a seed-based discovery run. Each call is a separate run.
pub fn request_discovery(conn: &Connection, connector: &str, limit: usize, now: i64) -> Result<String> {
    request_input(conn, connector, DiscoveryJob::Seeds, limit, now)
}

/// Queue a discovery run from seeds, one page or pasted text.
pub fn request_input(
    conn: &Connection,
    connector: &str,
    input: DiscoveryJob,
    limit: usize,
    now: i64,
) -> Result<String> {
    Ok(crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::DISCOVER,
            format!("discover:{}", new_id()),
            serde_json::json!({ "limit": limit, "input": input }),
        )
        .connector(connector),
        now,
    )?
    .id)
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
