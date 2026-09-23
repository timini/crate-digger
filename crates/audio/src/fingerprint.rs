//! Audio fingerprints (Chromaprint) and their comparison.
//!
//! Fingerprints identify audio: the same recording matches across formats,
//! bitrates and masters. A copy played at a different speed (a pitched
//! copy) matches once the speed is compensated, which is done by telling
//! the fingerprinter the audio's sample rate is scaled by that speed.

use std::path::Path;

use rusty_chromaprint::{match_fingerprints, Configuration, Fingerprinter};
use serde::{Deserialize, Serialize};

use crate::decode::{AudioError, Decoder};

/// Identifies the fingerprint algorithm and settings; stored with every
/// fingerprint so incompatible ones are never compared.
pub const ALGORITHM: &str = "chromaprint-test2";

fn config() -> Configuration {
    Configuration::preset_test2()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub algorithm: String,
    pub data: Vec<u32>,
    /// Length of the audio fingerprinted, after speed compensation.
    pub duration_ms: u64,
    /// The speed compensation applied (1.0 for none).
    pub speed: f64,
}

impl Fingerprint {
    /// Little-endian bytes for storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    pub fn from_bytes(bytes: &[u8], duration_ms: u64) -> Fingerprint {
        Fingerprint {
            algorithm: ALGORITHM.into(),
            data: bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            duration_ms,
            speed: 1.0,
        }
    }
}

/// Fingerprint interleaved samples. `speed` is how much faster than the
/// reference this audio plays; 1.04 means 4% fast.
pub fn fingerprint_samples(samples: &[f32], sample_rate: u32, channels: u16, speed: f64) -> Fingerprint {
    let mut fp = Streaming::new(sample_rate, channels, speed);
    fp.consume(samples);
    fp.finish()
}

/// Incremental fingerprinting, for decoding long files chunk by chunk.
pub struct Streaming {
    printer: Fingerprinter,
    buf: Vec<i16>,
    frames: u64,
    rate: u32,
    channels: u16,
    speed: f64,
}

impl Streaming {
    pub fn new(sample_rate: u32, channels: u16, speed: f64) -> Self {
        let mut printer = Fingerprinter::new(&config());
        let effective = ((sample_rate as f64) / speed).round().max(1.0) as u32;
        // Only fails for an unsupported rate or zero channels, which the
        // decoder never produces.
        let _ = printer.start(effective, channels.max(1) as u32);
        Streaming {
            printer,
            buf: Vec::new(),
            frames: 0,
            rate: sample_rate,
            channels: channels.max(1),
            speed,
        }
    }

    pub fn consume(&mut self, samples: &[f32]) {
        self.buf.clear();
        self.buf.extend(
            samples
                .iter()
                .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        );
        self.printer.consume(&self.buf);
        self.frames += (samples.len() / self.channels as usize) as u64;
    }

    pub fn finish(mut self) -> Fingerprint {
        self.printer.finish();
        Fingerprint {
            algorithm: ALGORITHM.into(),
            data: self.printer.fingerprint().to_vec(),
            duration_ms: (self.frames as f64 * 1000.0 * self.speed / self.rate.max(1) as f64) as u64,
            speed: self.speed,
        }
    }
}

/// Fingerprint a whole file.
pub fn fingerprint_file(path: &Path, speed: f64) -> Result<Fingerprint, AudioError> {
    let mut dec = Decoder::open(path)?;
    let mut buf = Vec::new();
    let mut fp: Option<Streaming> = None;
    while dec.next_chunk(&mut buf)? {
        let s = fp.get_or_insert_with(|| Streaming::new(dec.info.sample_rate, dec.info.channels, speed));
        s.consume(&buf);
    }
    fp.map(Streaming::finish)
        .ok_or_else(|| AudioError::Corrupt("no audio frames could be decoded".into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// 0 (no shared audio) to 1 (identical), over the aligned part.
    pub score: f64,
    /// Share of the longer recording covered by aligned audio. An excerpt or
    /// an edit scores well but covers only part of the full recording.
    pub coverage: f64,
    /// Where B's audio starts within A, in seconds (negative if earlier).
    pub offset_s: f64,
}

/// Bit errors per 32-bit item at which a segment counts as aligned. Random
/// audio averages about 16.
const MAX_SEGMENT_ERROR: f64 = 10.0;

pub fn compare(a: &Fingerprint, b: &Fingerprint) -> Comparison {
    let none = Comparison {
        score: 0.0,
        coverage: 0.0,
        offset_s: 0.0,
    };
    if a.algorithm != b.algorithm || a.data.is_empty() || b.data.is_empty() {
        return none;
    }
    let cfg = config();
    let Ok(segments) = match_fingerprints(&a.data, &b.data, &cfg) else {
        return none;
    };
    let aligned: Vec<_> = segments.iter().filter(|s| s.score <= MAX_SEGMENT_ERROR).collect();
    let items: usize = aligned.iter().map(|s| s.items_count).sum();
    if items == 0 {
        return none;
    }
    let weighted_error: f64 = aligned
        .iter()
        .map(|s| s.score * s.items_count as f64)
        .sum::<f64>()
        / items as f64;
    let longer = a.data.len().max(b.data.len()).max(1);
    let longest = aligned.iter().max_by_key(|s| s.items_count).unwrap();
    Comparison {
        score: (1.0 - weighted_error / 16.0).clamp(0.0, 1.0),
        coverage: (items as f64 / longer as f64).min(1.0),
        offset_s: (longest.start1(&cfg) - longest.start2(&cfg)) as f64,
    }
}

/// Compare B against A, also trying speed compensation around the speed
/// implied by their lengths. Returns the best comparison and its speed.
/// `b_at` fingerprints B at a given speed.
pub fn compare_with_speed(
    a: &Fingerprint,
    b_at: impl Fn(f64) -> Option<Fingerprint>,
    b_duration_ms: u64,
) -> Option<(Comparison, f64)> {
    let plain = b_at(1.0)?;
    let mut best = (compare(a, &plain), 1.0);
    // A strong match at normal speed needs no search. A weak alignment
    // spread over the whole track is typical of a slightly pitched copy.
    if best.0.score >= 0.6 && best.0.coverage >= 0.5 {
        return Some(best);
    }
    // A pitched copy's length changes by the speed: speed = len(A) / len(B).
    if a.duration_ms == 0 || b_duration_ms == 0 {
        return Some(best);
    }
    let implied = a.duration_ms as f64 / b_duration_ms as f64;
    if (implied - 1.0).abs() < 0.005 || (implied - 1.0).abs() > 0.12 {
        return Some(best);
    }
    for delta in [0.0, -0.003, 0.003] {
        let speed = implied + delta;
        if let Some(fp) = b_at(speed) {
            let c = compare(a, &fp);
            if c.score * c.coverage > best.0.score * best.0.coverage {
                best = (c, speed);
            }
        }
    }
    Some(best)
}
