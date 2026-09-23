//! Short-time spectral features, computed while the file streams past.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

pub const FRAME: usize = 2048;
pub const HOP: usize = 512;
pub const MEL_BANDS: usize = 32;
/// Chroma needs finer frequency resolution than the other features:
/// 8192 points gives 5.4 Hz bins at 44.1 kHz, narrower than a semitone
/// above about 90 Hz.
pub const CHROMA_FRAME: usize = 8192;
pub const CHROMA_HOP: usize = 2048;

#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub log_mel: [f32; MEL_BANDS],
    pub chroma: [f32; 12],
    /// Positive change in log-mel energy since the previous frame.
    pub onset: f32,
    pub rms: f32,
    /// Normalised to 0..1 of the Nyquist frequency.
    pub centroid: f32,
    pub rolloff: f32,
    pub flatness: f32,
}

pub struct FrameAnalyzer {
    rate: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    buf: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    mel: Vec<Vec<(usize, f32)>>,
    prev_log_mel: Option<[f32; MEL_BANDS]>,
    chroma_fft: Arc<dyn RealToComplex<f32>>,
    chroma_window: Vec<f32>,
    chroma_buf: Vec<f32>,
    chroma_input: Vec<f32>,
    chroma_spectrum: Vec<realfft::num_complex::Complex<f32>>,
    chroma_bin: Vec<Option<usize>>,
    current_chroma: [f32; 12],
    pub frames: Vec<Frame>,
    /// One chroma vector per CHROMA_HOP samples, with the frame's RMS.
    pub chroma_frames: Vec<([f32; 12], f32)>,
}

fn hz_to_mel(f: f32) -> f32 {
    2595.0 * (1.0 + f / 700.0).log10()
}

fn mel_to_hz(m: f32) -> f32 {
    700.0 * (10f32.powf(m / 2595.0) - 1.0)
}

impl FrameAnalyzer {
    pub fn new(rate: u32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
        let bins = FRAME / 2 + 1;
        let bin_hz = rate as f32 / FRAME as f32;
        let nyquist = rate as f32 / 2.0;

        // Triangular mel filters between 30 Hz and 16 kHz (or Nyquist).
        let (lo, hi) = (hz_to_mel(30.0), hz_to_mel(16_000f32.min(nyquist * 0.95)));
        let edges: Vec<f32> = (0..MEL_BANDS + 2)
            .map(|i| mel_to_hz(lo + (hi - lo) * i as f32 / (MEL_BANDS + 1) as f32))
            .collect();
        let mel = (0..MEL_BANDS)
            .map(|b| {
                let (l, c, r) = (edges[b], edges[b + 1], edges[b + 2]);
                (0..bins)
                    .filter_map(|k| {
                        let f = k as f32 * bin_hz;
                        let w = if f > l && f <= c {
                            (f - l) / (c - l)
                        } else if f > c && f < r {
                            (r - f) / (r - c)
                        } else {
                            0.0
                        };
                        (w > 0.0).then_some((k, w))
                    })
                    .collect()
            })
            .collect();

        let mut planner = RealFftPlanner::<f32>::new();
        let chroma_fft = planner.plan_fft_forward(CHROMA_FRAME);
        let chroma_hz = rate as f32 / CHROMA_FRAME as f32;
        // Pitch class of each fine bin between 80 Hz and 5 kHz; C = 0.
        let chroma_bin = (0..CHROMA_FRAME / 2 + 1)
            .map(|k| {
                let f = k as f32 * chroma_hz;
                (80.0..5000.0).contains(&f).then(|| {
                    let semis = 12.0 * (f / 261.625_58).log2();
                    (semis.round() as i32).rem_euclid(12) as usize
                })
            })
            .collect();
        let hann = |n: usize| -> Vec<f32> {
            (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
                .collect()
        };

        FrameAnalyzer {
            rate,
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            fft,
            window: hann(FRAME),
            buf: Vec::with_capacity(FRAME * 2),
            mel,
            prev_log_mel: None,
            chroma_input: chroma_fft.make_input_vec(),
            chroma_spectrum: chroma_fft.make_output_vec(),
            chroma_fft,
            chroma_window: hann(CHROMA_FRAME),
            chroma_buf: Vec::with_capacity(CHROMA_FRAME * 2),
            chroma_bin,
            current_chroma: [0.0; 12],
            frames: Vec::new(),
            chroma_frames: Vec::new(),
        }
    }

    pub fn frames_per_second(&self) -> f32 {
        self.rate as f32 / HOP as f32
    }

    /// Feed mono samples.
    pub fn push(&mut self, mono: &[f32]) {
        self.chroma_buf.extend_from_slice(mono);
        while self.chroma_buf.len() >= CHROMA_FRAME {
            self.analyse_chroma();
            self.chroma_buf.drain(..CHROMA_HOP);
        }
        self.buf.extend_from_slice(mono);
        while self.buf.len() >= FRAME {
            self.analyse_frame();
            self.buf.drain(..HOP);
        }
    }

    fn analyse_chroma(&mut self) {
        let mut energy = 0.0f32;
        for (i, (x, w)) in self.chroma_buf[..CHROMA_FRAME]
            .iter()
            .zip(&self.chroma_window)
            .enumerate()
        {
            self.chroma_input[i] = x * w;
            energy += x * x;
        }
        if self
            .chroma_fft
            .process(&mut self.chroma_input, &mut self.chroma_spectrum)
            .is_err()
        {
            return;
        }
        let mut chroma = [0f32; 12];
        for (k, pc) in self.chroma_bin.iter().enumerate() {
            if let Some(pc) = pc {
                chroma[*pc] += self.chroma_spectrum[k].norm();
            }
        }
        let cmax = chroma.iter().cloned().fold(0.0, f32::max);
        if cmax > 0.0 {
            chroma.iter_mut().for_each(|c| *c /= cmax);
        }
        self.current_chroma = chroma;
        self.chroma_frames
            .push((chroma, (energy / CHROMA_FRAME as f32).sqrt()));
    }

    fn analyse_frame(&mut self) {
        let mut energy = 0.0f32;
        for (i, (x, w)) in self.buf[..FRAME].iter().zip(&self.window).enumerate() {
            self.input[i] = x * w;
            energy += x * x;
        }
        let rms = (energy / FRAME as f32).sqrt();
        if self.fft.process(&mut self.input, &mut self.spectrum).is_err() {
            return;
        }
        let mags: Vec<f32> = self.spectrum.iter().map(|c| c.norm()).collect();

        let mut log_mel = [0f32; MEL_BANDS];
        for (b, filt) in self.mel.iter().enumerate() {
            let e: f32 = filt.iter().map(|(k, w)| mags[*k] * mags[*k] * w).sum();
            log_mel[b] = (e + 1e-10).ln();
        }
        let onset = match &self.prev_log_mel {
            Some(prev) => {
                log_mel
                    .iter()
                    .zip(prev)
                    .map(|(a, b)| (a - b).max(0.0))
                    .sum::<f32>()
                    / MEL_BANDS as f32
            }
            None => 0.0,
        };
        self.prev_log_mel = Some(log_mel);

        let chroma = self.current_chroma;

        let total: f32 = mags.iter().sum::<f32>().max(1e-10);
        let bins = mags.len() as f32;
        let centroid = mags.iter().enumerate().map(|(k, m)| k as f32 * m).sum::<f32>() / total / bins;
        let mut acc = 0.0;
        let mut rolloff = 1.0;
        for (k, m) in mags.iter().enumerate() {
            acc += m;
            if acc >= 0.85 * total {
                rolloff = k as f32 / bins;
                break;
            }
        }
        let log_mean = mags.iter().map(|m| (m + 1e-10).ln()).sum::<f32>() / bins;
        let flatness = (log_mean.exp() / (total / bins)).clamp(0.0, 1.0);

        self.frames.push(Frame {
            log_mel,
            chroma,
            onset,
            rms,
            centroid,
            rolloff,
            flatness,
        });
    }
}
