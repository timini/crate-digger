//! Stereo sample-rate conversion to the output device's rate.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

const CHUNK: usize = 1024;

/// Converts interleaved stereo from one rate to another. A pass-through
/// when the rates match.
pub struct StereoResampler {
    inner: Option<Fft<f32>>,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Leading frames of resampler delay still to drop.
    skip: usize,
    ratio: f64,
    frames_in: u64,
    frames_out: u64,
}

impl StereoResampler {
    pub fn new(from: u32, to: u32) -> Self {
        let inner = (from != to)
            .then(|| Fft::<f32>::new(from as usize, to as usize, CHUNK, 2, FixedSync::Input).ok())
            .flatten();
        let (out_max, skip) = inner
            .as_ref()
            .map(|r| (r.output_frames_max(), r.output_delay()))
            .unwrap_or((0, 0));
        StereoResampler {
            inner,
            input: Vec::with_capacity(CHUNK * 4),
            output: vec![0.0; out_max * 2],
            skip,
            ratio: to as f64 / from.max(1) as f64,
            frames_in: 0,
            frames_out: 0,
        }
    }

    pub fn reset(&mut self) {
        if let Some(r) = self.inner.as_mut() {
            r.reset();
            self.skip = r.output_delay();
        }
        self.input.clear();
        self.frames_in = 0;
        self.frames_out = 0;
    }

    fn run_chunk(&mut self, frames: usize, out: &mut Vec<f32>) {
        let Some(r) = self.inner.as_mut() else { return };
        let input = InterleavedSlice::new(&self.input[..], 2, self.input.len() / 2).expect("input size");
        let out_frames_cap = self.output.len() / 2;
        let mut output =
            InterleavedSlice::new_mut(&mut self.output[..], 2, out_frames_cap).expect("output size");
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: (frames < CHUNK).then_some(frames),
            active_channels_mask: None,
        };
        match r.process_into_buffer(&input, &mut output, Some(&indexing)) {
            Ok((_, produced)) => {
                let drop = self.skip.min(produced);
                self.skip -= drop;
                out.extend_from_slice(&self.output[drop * 2..produced * 2]);
                self.frames_out += (produced - drop) as u64;
            }
            Err(e) => tracing::warn!("resampler error: {e}"),
        }
    }

    /// Append the resampled version of `stereo` to `out`.
    pub fn process(&mut self, stereo: &[f32], out: &mut Vec<f32>) {
        if self.inner.is_none() {
            out.extend_from_slice(stereo);
            return;
        }
        self.input.extend_from_slice(stereo);
        self.frames_in += (stereo.len() / 2) as u64;
        while self.input.len() >= CHUNK * 2 {
            let rest = self.input.split_off(CHUNK * 2);
            self.run_chunk(CHUNK, out);
            self.input = rest;
        }
    }

    /// Flush buffered input at the end of a stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.inner.is_none() {
            return;
        }
        let start_len = out.len();
        let out_before = self.frames_out;
        let remaining = self.input.len() / 2;
        if remaining > 0 {
            self.input.resize(CHUNK * 2, 0.0);
            self.run_chunk(remaining, out);
        }
        // Push the delay line out with one chunk of silence.
        self.input.clear();
        self.input.resize(CHUNK * 2, 0.0);
        self.run_chunk(CHUNK, out);
        self.input.clear();
        // Drop the padding: emit exactly as many frames as the input implies.
        let expected = (self.frames_in as f64 * self.ratio).round() as u64;
        let allowed = expected.saturating_sub(out_before) as usize;
        out.truncate(start_len + allowed.min((out.len() - start_len) / 2) * 2);
        self.frames_out = out_before + ((out.len() - start_len) / 2) as u64;
    }
}

/// Mono conversion, built on the stereo resampler.
pub struct MonoResampler {
    inner: StereoResampler,
    stereo: Vec<f32>,
    out: Vec<f32>,
}

impl MonoResampler {
    pub fn new(from: u32, to: u32) -> Self {
        MonoResampler {
            inner: StereoResampler::new(from, to),
            stereo: Vec::new(),
            out: Vec::new(),
        }
    }

    fn fold(&mut self, out: &mut Vec<f32>) {
        out.extend(self.out.chunks_exact(2).map(|f| f[0]));
        self.out.clear();
    }

    pub fn process(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        self.stereo.clear();
        self.stereo.extend(mono.iter().flat_map(|s| [*s, *s]));
        let stereo = std::mem::take(&mut self.stereo);
        self.inner.process(&stereo, &mut self.out);
        self.stereo = stereo;
        self.fold(out);
    }

    pub fn finish(&mut self, out: &mut Vec<f32>) {
        self.inner.finish(&mut self.out);
        self.fold(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn mono_downsampling_keeps_length_ratio() {
        let mut r = MonoResampler::new(44_100, 16_000);
        let input: Vec<f32> = sine(44_100, 1.0).chunks(2).map(|f| f[0]).collect();
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.finish(&mut out);
        assert!((15_990..=16_010).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn same_rate_passes_through() {
        let mut r = StereoResampler::new(44_100, 44_100);
        let input = sine(44_100, 0.1);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        r.finish(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn converts_length_and_keeps_level() {
        let mut r = StereoResampler::new(44_100, 48_000);
        let input = sine(44_100, 1.0);
        let mut out = Vec::new();
        for chunk in input.chunks(1152 * 2) {
            r.process(chunk, &mut out);
        }
        r.finish(&mut out);
        let frames = out.len() / 2;
        assert!((47_990..=48_010).contains(&frames), "{frames} frames");
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.45..=0.55).contains(&peak), "peak {peak}");
    }
}
