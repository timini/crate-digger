//! Messages between the app and the analysis worker. One JSON object per
//! line: a request on the worker's stdin; heartbeats and one final result
//! or error on its stdout.

use serde::{Deserialize, Serialize};

use super::FeatureVersion;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Full analysis of a file.
    Analyse { path: String },
    /// Only a fingerprint, at a speed compensation (for pitched copies).
    Fingerprint { path: String, speed: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Sent regularly so the app can tell a slow analysis from a hung one.
    Heartbeat {
        decoded_ms: u64,
        peak_rss_kb: u64,
    },
    Analysis(Box<Analysis>),
    Fingerprint(FingerprintOut),
    Error {
        kind: ErrorKind,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    Unreadable,
    Unsupported,
    Corrupt,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintOut {
    pub algorithm: String,
    /// Little-endian u32 values, base64-free: plain numbers keep this simple.
    pub data: Vec<u32>,
    pub duration_ms: u64,
    pub speed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyEstimate {
    /// For example "A minor".
    pub name: String,
    /// Camelot notation, for example "8A".
    pub camelot: String,
    /// 0 to 1: how clearly this key beat the next best.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quality {
    /// Decoded length divided by the length the container states.
    pub decoded_fraction: Option<f32>,
    pub clipping_ratio: f32,
    pub silence_ratio: f32,
    pub decode_errors: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingOut {
    pub version: FeatureVersion,
    /// Index into `segments`, or None for the whole-track summary.
    pub segment: Option<usize>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub wall_ms: u64,
    pub peak_rss_kb: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub fingerprint: FingerprintOut,
    pub tempo_bpm: Option<f32>,
    pub tempo_confidence: f32,
    pub key: Option<KeyEstimate>,
    pub loudness_lufs: Option<f64>,
    pub quality: Quality,
    pub segments: Vec<Segment>,
    pub embeddings: Vec<EmbeddingOut>,
    /// Peak overview for display, 0..=255 per bin.
    pub waveform: Vec<u8>,
    pub stats: Stats,
}
