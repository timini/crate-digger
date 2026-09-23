//! Waveform overviews for display.

use std::path::Path;

use crate::decode::{AudioError, Decoder};

/// Frames summarised per intermediate block before resampling to `bins`.
const BLOCK: usize = 256;

/// Peak amplitude per bin, scaled to 0..=255, across the whole file.
pub fn peaks(path: &Path, bins: usize) -> Result<Vec<u8>, AudioError> {
    let mut dec = Decoder::open(path)?;
    let mut buf = Vec::new();
    let mut blocks: Vec<f32> = Vec::new();
    let mut current = 0.0f32;
    let mut in_block = 0usize;
    while dec.next_chunk(&mut buf)? {
        let ch = dec.info.channels.max(1) as usize;
        for frame in buf.chunks_exact(ch) {
            let v = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            current = current.max(v);
            in_block += 1;
            if in_block == BLOCK {
                blocks.push(current);
                current = 0.0;
                in_block = 0;
            }
        }
    }
    if in_block > 0 {
        blocks.push(current);
    }
    if blocks.is_empty() {
        return Err(AudioError::Corrupt("no audio frames could be decoded".into()));
    }
    let bins = bins.max(1);
    let out = (0..bins)
        .map(|b| {
            let start = b * blocks.len() / bins;
            let end = ((b + 1) * blocks.len() / bins).max(start + 1).min(blocks.len());
            let peak = blocks[start.min(blocks.len() - 1)..end]
                .iter()
                .fold(0.0f32, |m, v| m.max(*v));
            (peak.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    Ok(out)
}
