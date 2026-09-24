//! The contract between the Crate Digger app and its central service.
//!
//! Both sides use these types, and `tests/v1` pins their JSON form, so a
//! change to the wire format fails a test on both sides. The types carry
//! no local paths, ratings or credentials: shared contributions cannot
//! express them. Private backups have their own module and endpoints.
//!
//! Every request is validated with [`Validate`] by the service; the app
//! validates before sending so it can report problems without a round trip.

pub mod backup;

use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "v1";

/// Largest number of items in one lookup or submission.
pub const MAX_BATCH: usize = 100;
/// Largest request body the service accepts for shared data.
pub const MAX_BODY_BYTES: usize = 1 << 20;
pub const MAX_TEXT: usize = 300;
pub const MAX_EXTERNAL_IDS: usize = 10;

/// The analysis versions accepted for shared embeddings. Sharing is limited
/// to the pinned model (docs/decisions/0001-analysis-model.md); the app
/// checks this list against its own registry in a test.
pub const SHARED_FEATURE_VERSIONS: &[(FeatureVersionRef<'static>, usize)] = &[(
    FeatureVersionRef {
        model_id: "discogs-effnet-bsdynamic-1",
        weights_checksum: "sha256:a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c",
        preprocessing_version: "essentia-musicnn-input-16k-v1",
    },
    1280,
)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureVersionRef<'a> {
    pub model_id: &'a str,
    pub weights_checksum: &'a str,
    pub preprocessing_version: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureVersion {
    pub model_id: String,
    pub weights_checksum: String,
    pub preprocessing_version: String,
}

impl FeatureVersion {
    /// Expected dimensions if this version may be shared.
    pub fn shared_dims(&self) -> Option<usize> {
        SHARED_FEATURE_VERSIONS
            .iter()
            .find(|(v, _)| {
                v.model_id == self.model_id
                    && v.weights_checksum == self.weights_checksum
                    && v.preprocessing_version == self.preprocessing_version
            })
            .map(|(_, d)| *d)
    }
}

/// How a recording is identified across users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingKey {
    /// Hash of the leading Chromaprint values, as indexed by the app.
    pub fingerprint_hash: Option<String>,
    #[serde(default)]
    pub external_ids: Vec<ExternalId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalId {
    /// `discogs_release`, `discogs_track`, `musicbrainz_recording`, `isrc`.
    pub source: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub label: Option<String>,
    pub release: Option<String>,
    pub year: Option<i32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Features {
    pub version: FeatureVersion,
    pub embedding: Vec<f32>,
    pub tempo_bpm: Option<f32>,
    pub key_camelot: Option<String>,
    pub loudness_lufs: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    /// Only `youtube` in v1.
    pub kind: String,
    pub url: String,
}

/// One thing a user shares about one recording. The idempotency key is
/// chosen by the app and never reused, so a retried submission is
/// recognised as a duplicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub idempotency_key: String,
    pub recording: RecordingKey,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub features: Option<Features>,
    #[serde(default)]
    pub references: Vec<Reference>,
    /// A correction of earlier shared metadata. Kept beside what it
    /// disagrees with, never written over it.
    #[serde(default)]
    pub correction: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    pub contributions: Vec<Contribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Ack {
    Accepted {
        idempotency_key: String,
    },
    /// Already received earlier; nothing changed.
    Duplicate {
        idempotency_key: String,
    },
    Rejected {
        idempotency_key: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitResponse {
    pub acks: Vec<Ack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupRequest {
    pub recordings: Vec<RecordingKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogueEntry {
    pub recording_id: String,
    /// The metadata most contributors agree on.
    pub metadata: Metadata,
    /// Other values contributors gave, kept for reconciliation.
    pub alternatives: Vec<Metadata>,
    pub feature_versions: Vec<FeatureVersion>,
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupResponse {
    /// One entry per requested recording, in request order; `None` if unknown.
    pub results: Vec<Option<CatalogueEntry>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeaturesRequest {
    pub recording_id: String,
    pub version: FeatureVersion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeaturesResponse {
    pub features: Option<Features>,
    /// How many contributors' embeddings agree with the one returned.
    pub agreeing_contributors: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangesRequest {
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangesResponse {
    pub changes: Vec<CatalogueEntry>,
    pub next_cursor: Option<String>,
}

/// Why a request was refused. The service returns it as the error body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub message: String,
}

pub trait Validate {
    fn validate(&self) -> Result<(), Problem>;
}

fn problem(code: &str, message: impl Into<String>) -> Problem {
    Problem {
        code: code.into(),
        message: message.into(),
    }
}

fn text(field: &str, v: &Option<String>) -> Result<(), Problem> {
    match v {
        Some(s) if s.chars().count() > MAX_TEXT || s.chars().any(|c| c.is_control()) => Err(problem(
            "invalid_text",
            format!("{field} is too long or has control characters"),
        )),
        _ => Ok(()),
    }
}

impl Validate for RecordingKey {
    fn validate(&self) -> Result<(), Problem> {
        if self.fingerprint_hash.is_none() && self.external_ids.is_empty() {
            return Err(problem(
                "no_key",
                "a recording needs a fingerprint hash or an external id",
            ));
        }
        if self.external_ids.len() > MAX_EXTERNAL_IDS {
            return Err(problem("too_many_ids", "too many external ids"));
        }
        text("fingerprint_hash", &self.fingerprint_hash)?;
        for e in &self.external_ids {
            if !matches!(
                e.source.as_str(),
                "discogs_release" | "discogs_track" | "musicbrainz_recording" | "isrc"
            ) {
                return Err(problem(
                    "unknown_id_source",
                    format!("unknown id source {}", e.source),
                ));
            }
            text("id", &Some(e.id.clone()))?;
        }
        Ok(())
    }
}

impl Validate for Contribution {
    fn validate(&self) -> Result<(), Problem> {
        let key = &self.idempotency_key;
        if key.is_empty()
            || key.len() > 100
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(problem(
                "bad_idempotency_key",
                "the idempotency key must be 1 to 100 letters, digits, - or _",
            ));
        }
        self.recording.validate()?;
        if self.metadata.is_none() && self.features.is_none() && self.references.is_empty() {
            return Err(problem(
                "empty",
                "a contribution needs metadata, features or references",
            ));
        }
        if let Some(m) = &self.metadata {
            for (f, v) in [
                ("artist", &m.artist),
                ("title", &m.title),
                ("mix", &m.mix),
                ("label", &m.label),
                ("release", &m.release),
            ] {
                text(f, v)?;
            }
        }
        if let Some(f) = &self.features {
            let dims = f.version.shared_dims().ok_or_else(|| {
                problem(
                    "unsupported_model",
                    format!(
                        "features from {} are not accepted for sharing",
                        f.version.model_id
                    ),
                )
            })?;
            if f.embedding.len() != dims {
                return Err(problem(
                    "wrong_dimensions",
                    format!("expected {dims} values, got {}", f.embedding.len()),
                ));
            }
            if f.embedding.iter().any(|v| !v.is_finite()) {
                return Err(problem("not_finite", "embedding values must be finite numbers"));
            }
            text("key_camelot", &f.key_camelot)?;
        }
        if self.references.len() > 5 {
            return Err(problem("too_many_references", "at most 5 references"));
        }
        for r in &self.references {
            if r.kind != "youtube"
                || !r.url.starts_with("https://www.youtube.com/watch?v=")
                || r.url.len() > 60
            {
                return Err(problem("bad_reference", "only YouTube watch links are accepted"));
            }
        }
        Ok(())
    }
}

impl Validate for SubmitRequest {
    fn validate(&self) -> Result<(), Problem> {
        if self.contributions.is_empty() || self.contributions.len() > MAX_BATCH {
            return Err(problem(
                "batch_size",
                format!("send 1 to {MAX_BATCH} contributions"),
            ));
        }
        Ok(())
    }
}

impl Validate for LookupRequest {
    fn validate(&self) -> Result<(), Problem> {
        if self.recordings.is_empty() || self.recordings.len() > MAX_BATCH {
            return Err(problem(
                "batch_size",
                format!("look up 1 to {MAX_BATCH} recordings"),
            ));
        }
        self.recordings.iter().try_for_each(Validate::validate)
    }
}

impl Validate for ChangesRequest {
    fn validate(&self) -> Result<(), Problem> {
        if self.limit == 0 || self.limit as usize > MAX_BATCH {
            return Err(problem("batch_size", format!("ask for 1 to {MAX_BATCH} changes")));
        }
        text("cursor", &self.cursor)
    }
}
