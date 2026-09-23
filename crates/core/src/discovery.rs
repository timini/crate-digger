//! Turning source proposals into candidates.
//!
//! Every candidate keeps its evidence: where it came from, when, and the
//! supporting excerpt. Only candidates backed by retrievable evidence are
//! marked verified, and only verified candidates may be acquired
//! automatically.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{AdapterError, CandidateProposal, DiscoverySource, Seed};
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
        if known_track(&tx, p)?.is_some() {
            summary.already_known += 1;
            continue;
        }
        let track_id = meta::create_track(&tx)?;
        meta::set_extracted(
            &tx,
            &track_id,
            source_id,
            &[
                (Field::Artist, Some(p.artist.clone())),
                (Field::Title, Some(p.title.clone())),
                (Field::Mix, p.mix.clone()),
                (Field::Label, p.label.clone()),
                (Field::Release, p.release.clone()),
            ],
        )?;
        let verified = p
            .evidence
            .iter()
            .any(|e| e.source_url.is_some() || e.supplied_text_id.is_some());
        let confidence = p.evidence.iter().map(|e| e.confidence).fold(0.0f64, f64::max);
        let candidate_id = new_id();
        tx.execute(
            "INSERT INTO candidate (id, track_id, stage, verified, confidence, score, created_at, updated_at)
             VALUES (?1, ?2, 'candidate', ?3, ?4, ?4, ?5, ?5)",
            params![candidate_id, track_id, verified, confidence, now],
        )?;
        for e in &p.evidence {
            tx.execute(
                "INSERT INTO evidence (id, candidate_id, source_kind, source_url, supplied_text_id,
                                       retrieved_at, excerpt, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id(),
                    candidate_id,
                    e.source_kind,
                    e.source_url,
                    e.supplied_text_id,
                    now,
                    e.excerpt,
                    e.confidence
                ],
            )?;
        }
        for reason in &p.reasons {
            tx.execute(
                "INSERT INTO explanation (id, candidate_id, reason) VALUES (?1, ?2, ?3)",
                params![new_id(), candidate_id, reason],
            )?;
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

#[derive(Serialize, Deserialize)]
struct DiscoverPayload {
    limit: usize,
}

/// Asks a source for candidates, stores them with their evidence and
/// queues acquisition for the verified ones.
pub struct DiscoverHandler {
    pub source: Arc<dyn DiscoverySource>,
    pub acquirer_id: String,
}

impl Handler for DiscoverHandler {
    fn kind(&self) -> &'static str {
        kinds::DISCOVER
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let payload: DiscoverPayload = ctx.payload()?;
        let seeds = seeds(ctx.conn)?;
        let proposals = match self.source.discover(&seeds, payload.limit) {
            Ok(p) => p,
            Err(e @ AdapterError::Auth(_)) => return Err(ctx.adapter_error(e)),
            Err(e) => return Err(ctx.adapter_error(e)),
        };
        let now = ctx.now();
        let summary = ingest(ctx.conn, self.source.id(), &proposals, now)?;
        for id in &summary.created {
            identify(ctx.conn, id, now)?;
            let verified: bool = ctx
                .conn
                .query_row("SELECT verified FROM candidate WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .map_err(Error::from)?;
            if verified {
                queue_acquisition(ctx.conn, id, &self.acquirer_id, now)?;
            }
        }
        ctx.save_checkpoint(&summary)
    }
}

/// Queue a discovery run. Each call is a separate run.
pub fn request_discovery(conn: &Connection, connector: &str, limit: usize, now: i64) -> Result<String> {
    Ok(crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::DISCOVER,
            format!("discover:{}", new_id()),
            serde_json::json!({ "limit": limit }),
        )
        .connector(connector),
        now,
    )?
    .id)
}
