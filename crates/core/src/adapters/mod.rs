//! Interfaces to everything outside the app: discovery sources, LLM
//! providers, audio acquisition and the central service.
//!
//! Traits are synchronous; callers run them on worker threads. Every
//! adapter reports failures as [`AdapterError`] so the job system can tell
//! authentication problems, outages and bad input apart.

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

/// Tier 1 discovery: turns seeds into candidates with evidence.
pub trait DiscoverySource: Send + Sync {
    fn id(&self) -> &str;
    fn discover(&self, seeds: &[Seed], limit: usize) -> AdapterResult<Vec<CandidateProposal>>;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub result_id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub duration_ms: Option<u64>,
    pub format: Option<String>,
    pub bitrate_kbps: Option<u32>,
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
