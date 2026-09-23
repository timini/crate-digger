//! Audio fingerprints (Chromaprint). Comparing them is identity logic and
//! lives in `cd_core::identity::fingerprint`.
//!
//! A copy played at a different speed (a pitched copy) matches once the
//! speed is compensated, which is done by telling the fingerprinter the
//! audio's sample rate is scaled by that speed.

use std::path::Path;

use rusty_chromaprint::{Configuration, Fingerprinter};
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
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
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
