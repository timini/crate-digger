//! Interfaces to everything outside the app: discovery sources, LLM
//! providers, audio acquisition and the central service.
//!
//! Traits are synchronous; callers run them on worker threads. Every
//! adapter reports failures as [`AdapterError`] so the job system can tell
//! authentication problems, outages and bad input apart.

pub mod demo;
pub mod fake;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::SeedKind;

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum AdapterError {
    /// Credentials missing or rejected. The connector pauses until fixed.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Service unreachable or erroring. Retried with backoff.
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("rate limited")]
    RateLimited { retry_after_ms: Option<u64> },
    /// The request or response was invalid. Not retried.
    #[error("invalid: {0}")]
    Invalid(String),
}

impl AdapterError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AdapterError::Unavailable(_) | AdapterError::RateLimited { .. }
        )
    }
}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seed {
    pub kind: SeedKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceProposal {
    pub source_kind: String,
    pub source_url: Option<String>,
    pub supplied_text_id: Option<String>,
    pub excerpt: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateProposal {
    pub artist: String,
    pub title: String,
    pub mix: Option<String>,
    pub label: Option<String>,
    pub release: Option<String>,
    pub reasons: Vec<String>,
    pub evidence: Vec<EvidenceProposal>,
}

/// Evidence of this kind comes from a model alone and never verifies a candidate.
pub const LLM_EVIDENCE: &str = "llm";

/// What a discovery run works from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveryInput {
    /// Expand saved seeds and positively rated tracks.
    Seeds,
    /// One public page the user supplied.
    Page { url: String },
    /// Text the user pasted, stored as `supplied_text`.
    Text { supplied_text_id: String, text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryRequest {
    pub seeds: Vec<Seed>,
    pub limit: usize,
    pub input: DiscoveryInput,
}

/// Tier 1 discovery: turns seeds, pages or pasted text into candidates with evidence.
pub trait DiscoverySource: Send + Sync {
    fn id(&self) -> &str;
    fn discover(&self, request: &DiscoveryRequest) -> AdapterResult<Vec<CandidateProposal>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredRequest {
    pub instructions: String,
    /// Untrusted content (page text, pasted text). Always passed as data.
    pub data: String,
    pub json_schema: serde_json::Value,
}

pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    fn supports_structured_output(&self) -> bool;
    fn complete_json(&self, req: &StructuredRequest) -> AdapterResult<serde_json::Value>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionQuery {
    pub artist: String,
    pub title: String,
    pub mix: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub result_id: String,
    /// The full path as the source shares it.
    pub filename: String,
    pub size_bytes: u64,
    pub duration_ms: Option<u64>,
    pub format: Option<String>,
    pub bitrate_kbps: Option<u32>,
    /// Who shares it, and how soon they could send it.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub free_slot: Option<bool>,
    #[serde(default)]
    pub queue_length: Option<u64>,
    #[serde(default)]
    pub upload_speed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TransferStatus {
    Queued,
    InProgress { bytes: u64, total: u64 },
    Completed { path: PathBuf },
    Failed { reason: String },
    Cancelled,
}

/// Audio acquisition (slskd in milestone 3).
pub trait Acquirer: Send + Sync {
    fn id(&self) -> &str;
    /// True when results are made for the exact query (demo and test
    /// sources), so the matching rule and the user's choice are skipped.
    fn exact_results(&self) -> bool {
        false
    }
    fn search(&self, query: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>>;
    /// Start a transfer into `dest_dir`. Calling again with the same
    /// idempotency key returns the existing transfer instead of starting a
    /// second one.
    fn enqueue(
        &self,
        result: &SearchResult,
        idempotency_key: &str,
        dest_dir: &std::path::Path,
    ) -> AdapterResult<String>;
    fn status(&self, transfer_id: &str) -> AdapterResult<TransferStatus>;
    fn cancel(&self, transfer_id: &str) -> AdapterResult<()>;
}

/// A YouTube video that plausibly is this track, already confirmed to exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoMatch {
    pub video_id: String,
    pub url: String,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub duration_ms: Option<i64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoQuery {
    pub artist: String,
    pub title: String,
    pub mix: Option<String>,
    /// Known length of the recording, from a local file.
    pub duration_ms: Option<i64>,
}

/// Reference links (YouTube in v1). Links are references, never audio sources.
pub trait VideoLookup: Send + Sync {
    fn id(&self) -> &str;
    fn lookup(&self, query: &VideoQuery) -> AdapterResult<Vec<VideoMatch>>;
}

/// What is known about a library track when looking up its metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataQuery {
    /// Chromaprint values (AcoustID's algorithm) and the track's length.
    pub fingerprint: Vec<u32>,
    pub duration_ms: i64,
    pub artist: Option<String>,
    pub title: Option<String>,
}

/// One possible identity for a track, from fingerprint lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identified {
    /// AcoustID's match score, 0 to 1.
    pub score: f64,
    /// The identified recording's length, when known.
    pub duration_ms: Option<i64>,
    /// Field name (as in `domain::Field`) and value, from MusicBrainz.
    pub fields: Vec<(String, String)>,
    /// Extra fields from Discogs, such as genre and label.
    pub discogs_fields: Vec<(String, String)>,
    /// (source, id), for example ("musicbrainz_recording", "...").
    pub external_ids: Vec<(String, String)>,
}

/// Library metadata lookup (AcoustID, MusicBrainz and Discogs in v1).
pub trait MetadataLookup: Send + Sync {
    fn lookup(&self, query: &MetadataQuery) -> AdapterResult<Vec<Identified>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncAck {
    pub idempotency_key: String,
    /// True when the service had already accepted this key earlier.
    pub duplicate: bool,
}

/// Central metadata service (milestone 4).
pub trait CentralSync: Send + Sync {
    fn submit(
        &self,
        idempotency_key: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> AdapterResult<SyncAck>;
}
