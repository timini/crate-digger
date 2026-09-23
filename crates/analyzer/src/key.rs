//! Musical key from average chroma, using the Krumhansl-Kessler profiles.

use cd_core::analysis::protocol::KeyEstimate;

const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

fn pearson(a: &[f32; 12], b: &[f32]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da == 0.0 || db == 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// Camelot wheel position: major keys are B, minor keys A. C major is 8B,
/// A minor 8A.
pub fn camelot(root: usize, minor: bool) -> String {
    let major_root = if minor { (root + 3) % 12 } else { root };
    let n = (major_root * 7) % 12;
    format!("{}{}", (n + 7) % 12 + 1, if minor { 'A' } else { 'B' })
}

/// Key from chroma frames (see `FrameAnalyzer::chroma_frames`).
pub fn estimate(chroma_frames: &[([f32; 12], f32)]) -> Option<KeyEstimate> {
    let mut chroma = [0f32; 12];
    let mut used = 0;
    for (c, _) in chroma_frames.iter().filter(|(_, rms)| *rms > 1e-3) {
        for (acc, v) in chroma.iter_mut().zip(c) {
            *acc += v;
        }
        used += 1;
    }
    if used < 8 {
        return None;
    }
    let mut scores: Vec<(f32, usize, bool)> = Vec::with_capacity(24);
    for root in 0..12 {
        let rotated: Vec<f32> = (0..12).map(|i| chroma[(i + root) % 12]).collect();
        scores.push((pearson(&MAJOR, &rotated), root, false));
        scores.push((pearson(&MINOR, &rotated), root, true));
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, root, minor) = scores[0];
    if best <= 0.0 {
        return None;
    }
    Some(KeyEstimate {
        name: format!("{} {}", NAMES[root], if minor { "minor" } else { "major" }),
        camelot: camelot(root, minor),
        confidence: ((best - scores[1].0) / best).clamp(0.0, 1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_positions() {
        assert_eq!(camelot(0, false), "8B"); // C major
        assert_eq!(camelot(9, true), "8A"); // A minor
        assert_eq!(camelot(7, false), "9B"); // G major
        assert_eq!(camelot(4, true), "9A"); // E minor
        assert_eq!(camelot(5, false), "7B"); // F major
        assert_eq!(camelot(2, true), "7A"); // D minor
        assert_eq!(camelot(6, false), "2B"); // F# major
        assert_eq!(camelot(3, true), "2A"); // Eb minor
    }
}
