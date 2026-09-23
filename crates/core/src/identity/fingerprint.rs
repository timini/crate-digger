//! Comparing Chromaprint fingerprints.
//!
//! Fingerprints identify audio: the same recording matches across formats,
//! bitrates and masters. Produced by the audio crate; compared here.

use rusty_chromaprint::{match_fingerprints, Configuration};
use serde::{Deserialize, Serialize};

use crate::analysis::protocol::FingerprintOut;

/// Must match the audio crate's fingerprinting configuration.
fn config() -> Configuration {
    Configuration::preset_test2()
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

pub fn compare(a: &FingerprintOut, b: &FingerprintOut) -> Comparison {
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

/// Speed ratios worth trying for B against A, implied by their lengths, if
/// they differ by more than 0.5% and less than 12% (a DJ pitch range).
pub fn implied_speeds(a_duration_ms: u64, b_duration_ms: u64) -> Vec<f64> {
    if a_duration_ms == 0 || b_duration_ms == 0 {
        return Vec::new();
    }
    let implied = a_duration_ms as f64 / b_duration_ms as f64;
    if (implied - 1.0).abs() < 0.005 || (implied - 1.0).abs() > 0.12 {
        return Vec::new();
    }
    vec![implied, implied - 0.003, implied + 0.003]
}

/// Compare B against A, also trying speed compensation around the speed
/// implied by their lengths. `b_at` fingerprints B at a given speed.
/// Returns the best comparison and its speed.
pub fn compare_with_speed(
    a: &FingerprintOut,
    b_at: impl Fn(f64) -> Option<FingerprintOut>,
    b_duration_ms: u64,
) -> Option<(Comparison, f64)> {
    let plain = b_at(1.0)?;
    let mut best = (compare(a, &plain), 1.0);
    // A strong match at normal speed needs no search. A weak alignment
    // spread over the whole track is typical of a slightly pitched copy.
    if best.0.score >= 0.6 && best.0.coverage >= 0.5 {
        return Some(best);
    }
    for speed in implied_speeds(a.duration_ms, b_duration_ms) {
        if let Some(fp) = b_at(speed) {
            let c = compare(a, &fp);
            if c.score * c.coverage > best.0.score * best.0.coverage {
                best = (c, speed);
            }
        }
    }
    Some(best)
}
