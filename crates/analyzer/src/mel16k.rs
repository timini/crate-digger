//! Essentia's "MusiCNN input" features, which Discogs-EffNet and MusiCNN
//! were trained on: 16 kHz mono, 512-sample Hann frames with a 256-sample
//! hop, magnitude spectrum, 96 Slaney mel bands from 0 to 8 kHz with
//! unit-area triangles, then log10(1 + 10000 x).
//!
//! Reimplemented from Essentia's TensorflowInputMusiCNN; the evaluation
//! checks the embeddings behave as expected on real music.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

pub const RATE: u32 = 16_000;
pub const FRAME: usize = 512;
pub const HOP: usize = 256;
pub const BANDS: usize = 96;
/// Names this preprocessing in feature versions.
pub const PREPROCESSING: &str = "essentia-musicnn-input-16k-v1";

fn hz_to_slaney(f: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if f < min_log_hz {
        f / f_sp
    } else {
        min_log_mel + (f / min_log_hz).ln() / logstep
    }
}

fn slaney_to_hz(m: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if m < min_log_mel {
        m * f_sp
    } else {
        min_log_hz * ((m - min_log_mel) * logstep).exp()
    }
}

pub struct MelFrames {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    filters: Vec<Vec<(usize, f32)>>,
    buf: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    /// One row of BANDS values per frame.
    pub frames: Vec<[f32; BANDS]>,
}

impl Default for MelFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl MelFrames {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
        let bins = FRAME / 2 + 1;
        let bin_hz = RATE as f64 / FRAME as f64;
        let (lo, hi) = (hz_to_slaney(0.0), hz_to_slaney(8000.0));
        let edges: Vec<f64> = (0..BANDS + 2)
            .map(|i| slaney_to_hz(lo + (hi - lo) * i as f64 / (BANDS + 1) as f64))
            .collect();
        let filters = (0..BANDS)
            .map(|b| {
                let (l, c, r) = (edges[b], edges[b + 1], edges[b + 2]);
                let area_norm = 2.0 / (r - l);
                (0..bins)
                    .filter_map(|k| {
                        let f = k as f64 * bin_hz;
                        let w = if f > l && f <= c {
                            (f - l) / (c - l)
                        } else if f > c && f < r {
                            (r - f) / (r - c)
                        } else {
                            0.0
                        };
                        (w > 0.0).then_some((k, (w * area_norm) as f32))
                    })
                    .collect()
            })
            .collect();
        // Symmetric Hann, as Essentia's Windowing uses.
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FRAME - 1) as f32).cos())
            .collect();
        MelFrames {
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            fft,
            window,
            filters,
            buf: Vec::with_capacity(FRAME * 2),
            frames: Vec::new(),
        }
    }

    /// Feed 16 kHz mono samples.
    pub fn push(&mut self, mono16k: &[f32]) {
        self.buf.extend_from_slice(mono16k);
        while self.buf.len() >= FRAME {
            for (i, (x, w)) in self.buf[..FRAME].iter().zip(&self.window).enumerate() {
                self.input[i] = x * w;
            }
            if self.fft.process(&mut self.input, &mut self.spectrum).is_ok() {
                let mut row = [0f32; BANDS];
                for (b, filt) in self.filters.iter().enumerate() {
                    let e: f32 = filt.iter().map(|(k, w)| self.spectrum[*k].norm() * w).sum();
                    row[b] = (1.0 + 10_000.0 * e).log10();
                }
                self.frames.push(row);
            }
            self.buf.drain(..HOP);
        }
    }

    pub fn frames_per_second() -> f32 {
        RATE as f32 / HOP as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slaney_scale_round_trips() {
        for f in [0.0, 440.0, 1000.0, 4000.0, 8000.0] {
            assert!((slaney_to_hz(hz_to_slaney(f)) - f).abs() < 1e-6);
        }
    }

    #[test]
    fn a_tone_lights_up_the_right_band() {
        let tone: Vec<f32> = (0..16_000)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let mut m = MelFrames::new();
        m.push(&tone);
        assert_eq!(m.frames.len(), (16_000 - FRAME) / HOP + 1);
        let row = m.frames[10];
        let peak = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        // The Slaney scale is linear below 1 kHz, which takes about a third
        // of the mel range up to 8 kHz: 1 kHz falls near band 31 of 96.
        assert!((30..=32).contains(&peak), "peak band {peak}");
    }
}
