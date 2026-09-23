//! Deterministic synthetic songs for identity and analysis tests.
//!
//! A seed picks tempo, key, chord progression, melody and drum pattern. The
//! same seed always renders the same audio, so tests can create a recording
//! and its variants (remaster, section, edit, pitched copy, remix) without
//! committing large files. This is test material, not music.

use std::path::Path;

/// Interleaved stereo audio at [`RATE`].
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub samples: Vec<f32>,
}

pub const RATE: u32 = 44_100;
const CH: usize = 2;

impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / CH
    }

    pub fn duration_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / RATE as u64
    }

    /// Write 16-bit PCM WAV.
    pub fn write_wav(&self, path: &Path) -> std::io::Result<()> {
        let data_len = (self.samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(CH as u16).to_le_bytes());
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&(RATE * CH as u32 * 2).to_le_bytes());
        out.extend_from_slice(&((CH * 2) as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
        }
        std::fs::write(path, out)
    }
}

/// SplitMix64: small, deterministic, good enough for test material.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Everything that makes a song recognisably itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub bpm: f32,
    /// Semitones above A.
    pub key: i32,
    /// Chord roots in semitones from the key, one per bar, repeating.
    pub progression: Vec<i32>,
    /// Melody: scale degree per sixteenth step, or None for a rest. 32 steps.
    pub melody: Vec<Option<i32>>,
    /// Drum pattern per 16 steps: (kick, snare, hat).
    pub drums: Vec<(bool, bool, bool)>,
    /// Bass rhythm: which sixteenth steps play.
    pub bass: Vec<bool>,
    pub seed: u64,
}

const MINOR_SCALE: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

impl Song {
    pub fn from_seed(seed: u64) -> Song {
        let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x00C0_FFEE);
        let bpm = 116.0 + r.below(16) as f32;
        let key = r.below(12) as i32;
        let progression = (0..4).map(|_| MINOR_SCALE[r.below(7) as usize]).collect();
        let melody = (0..32)
            .map(|_| (r.below(3) != 0).then(|| r.below(10) as i32))
            .collect();
        Song {
            bpm,
            key,
            progression,
            melody,
            drums: drum_pattern(&mut r),
            bass: (0..16).map(|i| i % 4 == 2 || r.below(4) == 0).collect(),
            seed,
        }
    }

    /// Same key, chords and melody; new drums, bass rhythm and tempo.
    pub fn remix(&self, remix_seed: u64) -> Song {
        let mut r = Rng(self.seed ^ remix_seed.wrapping_mul(0x9E37_79B9));
        Song {
            bpm: self.bpm + 4.0 + r.below(6) as f32,
            drums: drum_pattern(&mut r),
            bass: (0..16).map(|i| i % 2 == 0 || r.below(3) == 0).collect(),
            seed: self.seed ^ remix_seed,
            ..self.clone()
        }
    }

    pub fn render(&self, seconds: f32) -> Audio {
        render(self, seconds)
    }
}

fn drum_pattern(r: &mut Rng) -> Vec<(bool, bool, bool)> {
    (0..16)
        .map(|i| {
            let kick = i % 4 == 0 || r.below(8) == 0;
            let snare = i % 8 == 4 || r.below(12) == 0;
            let hat = i % 2 == 1 || r.below(3) == 0;
            (kick, snare, hat)
        })
        .collect()
}

fn freq(semitones_from_a: i32, octave_shift: i32) -> f32 {
    220.0 * 2f32.powf((semitones_from_a + 12 * octave_shift) as f32 / 12.0)
}

fn render(song: &Song, seconds: f32) -> Audio {
    let frames = (seconds * RATE as f32) as usize;
    let mut out = vec![0f32; frames * CH];
    let step = 60.0 / song.bpm / 4.0; // a sixteenth note
    let mut noise = Rng(song.seed ^ 0xABCD);
    let steps = (seconds / step) as usize + 1;
    for s in 0..steps {
        let t0 = s as f32 * step;
        let bar = s / 16;
        let in_bar = s % 16;
        let chord_root = song.key + song.progression[bar % song.progression.len()];
        // Vary the melody every eight bars so sections differ from each other.
        let section = (bar / 8) as i32;
        let (kick, snare, hat) = song.drums[in_bar];
        let mut add = |start: f32, len: f32, f: &mut dyn FnMut(f32) -> f32, pan: f32| {
            let a = (start * RATE as f32) as usize;
            let n = (len * RATE as f32) as usize;
            for i in 0..n {
                let idx = a + i;
                if idx >= frames {
                    break;
                }
                let v = f(i as f32 / RATE as f32);
                out[idx * 2] += v * (1.0 - pan);
                out[idx * 2 + 1] += v * (1.0 + pan);
            }
        };
        if kick {
            add(
                t0,
                0.25,
                &mut |t| {
                    (std::f32::consts::TAU * (50.0 + 70.0 * (-t * 30.0).exp()) * t).sin()
                        * (-t * 12.0).exp()
                        * 0.5
                },
                0.0,
            );
        }
        if snare {
            let mut n = Rng(noise.next());
            add(
                t0,
                0.18,
                &mut |t| {
                    ((n.unit() - 0.5) * 0.6 + (std::f32::consts::TAU * 190.0 * t).sin() * 0.2)
                        * (-t * 22.0).exp()
                },
                0.1,
            );
        }
        if hat {
            let mut n = Rng(noise.next());
            let mut prev = 0.0;
            add(
                t0,
                0.05,
                &mut |t| {
                    let x = n.unit() - 0.5;
                    let hp = x - prev;
                    prev = x;
                    hp * 0.25 * (-t * 70.0).exp()
                },
                -0.3,
            );
        }
        if song.bass[in_bar] {
            let f = freq(chord_root, -2);
            add(
                t0,
                step * 0.9,
                &mut |t| {
                    let ph = std::f32::consts::TAU * f * t;
                    (ph.sin() + 0.4 * (2.0 * ph).sin() + 0.2 * (3.0 * ph).sin()) * 0.22 * (-t * 4.0).exp()
                },
                0.0,
            );
        }
        if in_bar == 0 {
            // A minor triad pad for the whole bar.
            for (i, iv) in [0, 3, 7].iter().enumerate() {
                let f = freq(chord_root + iv, 0);
                add(
                    t0,
                    step * 16.0,
                    &mut |t| (std::f32::consts::TAU * f * t).sin() * 0.06 * (1.0 - (-t * 8.0).exp()),
                    (i as f32 - 1.0) * 0.4,
                );
            }
        }
        let m = song.melody[(s + section as usize * 5) % song.melody.len()];
        if let Some(degree) = m {
            let semis = MINOR_SCALE[(degree as usize) % 7] + 12 * (degree / 7) + section % 2 * 2;
            let f = freq(song.key + semis, 1);
            add(
                t0,
                step * 0.8,
                &mut |t| {
                    let ph = (f * t).fract();
                    (if ph < 0.5 { 0.12 } else { -0.12 }) * (-t * 6.0).exp()
                },
                0.2,
            );
        }
    }
    // Leave headroom.
    let peak = out.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.8 / peak;
    out.iter_mut().for_each(|v| *v *= gain);
    Audio { samples: out }
}

/// A different master of the same recording: tilted EQ, lower level and
/// gentle saturation.
pub fn remaster(a: &Audio) -> Audio {
    let mut out = Vec::with_capacity(a.samples.len());
    let mut low = [0f32; CH];
    for (i, s) in a.samples.iter().enumerate() {
        let c = i % CH;
        low[c] += 0.05 * (s - low[c]);
        let shelved = s + 0.6 * low[c];
        out.push((shelved * 1.4).tanh() * 0.6);
    }
    Audio { samples: out }
}

/// The part between two fractions of the length.
pub fn section(a: &Audio, from: f32, to: f32) -> Audio {
    let f = a.frames();
    let (s, e) = ((from * f as f32) as usize, (to * f as f32) as usize);
    Audio {
        samples: a.samples[s * CH..e.min(f) * CH].to_vec(),
    }
}

/// Remove the part between two fractions of the length (an edit).
pub fn cut(a: &Audio, from: f32, to: f32) -> Audio {
    let f = a.frames();
    let (s, e) = ((from * f as f32) as usize, (to * f as f32) as usize);
    let mut samples = a.samples[..s * CH].to_vec();
    samples.extend_from_slice(&a.samples[e.min(f) * CH..]);
    Audio { samples }
}

/// Play `speed` times faster, changing pitch and tempo together, as a
/// turntable or a pitched digital copy does.
pub fn speed(a: &Audio, speed: f64) -> Audio {
    let f = a.frames();
    let n = (f as f64 / speed) as usize;
    let mut samples = Vec::with_capacity(n * CH);
    for i in 0..n {
        let pos = i as f64 * speed;
        let j = pos as usize;
        let frac = (pos - j as f64) as f32;
        for c in 0..CH {
            let x0 = a.samples[j.min(f - 1) * CH + c];
            let x1 = a.samples[(j + 1).min(f - 1) * CH + c];
            samples.push(x0 + (x1 - x0) * frac);
        }
    }
    Audio { samples }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_are_deterministic_and_distinct() {
        let a = Song::from_seed(1).render(2.0);
        let b = Song::from_seed(1).render(2.0);
        let c = Song::from_seed(2).render(2.0);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.duration_ms(), 2000);
        let peak = a.samples.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((0.7..=0.81).contains(&peak));
    }

    #[test]
    fn variants_have_the_expected_lengths() {
        let a = Song::from_seed(3).render(10.0);
        assert_eq!(section(&a, 0.25, 0.75).duration_ms(), 5000);
        assert_eq!(cut(&a, 0.4, 0.6).duration_ms(), 8000);
        let fast = speed(&a, 1.04);
        assert!((fast.duration_ms() as i64 - 9615).abs() <= 2);
        assert_eq!(remaster(&a).frames(), a.frames());
    }

    #[test]
    fn a_remix_keeps_key_chords_and_melody() {
        let s = Song::from_seed(4);
        let r = s.remix(1);
        assert_eq!(
            (s.key, &s.progression, &s.melody),
            (r.key, &r.progression, &r.melody)
        );
        assert_ne!(s.bpm, r.bpm);
    }
}
