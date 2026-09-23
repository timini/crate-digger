//! Demo acquisition: generates short tone recordings instead of
//! downloading, so the review flow can be tried before real sources exist.
//! Only used when the user turns on demo discovery.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;

/// Writes a 16-bit stereo WAV: two tones with a pulse at `bpm`.
pub fn write_tone_wav(path: &Path, seed: u64, seconds: u32) -> std::io::Result<()> {
    const RATE: u32 = 44_100;
    let base = 110.0 * 2f32.powf((seed % 24) as f32 / 12.0);
    let bpm = 118.0 + (seed % 12) as f32;
    let frames = RATE * seconds;
    let data_len = frames * 4;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    let beat = 60.0 / bpm;
    for i in 0..frames {
        let t = i as f32 / RATE as f32;
        let phase = (t % beat) / beat;
        let pulse = (-phase * 6.0).exp();
        let fade = (t.min(seconds as f32 - t) / 0.5).min(1.0);
        let tone = (t * base * std::f32::consts::TAU).sin() * 0.35
            + (t * base * 1.5 * std::f32::consts::TAU).sin() * 0.2;
        let v = (tone * (0.35 + 0.65 * pulse) * fade * 0.8 * i16::MAX as f32) as i16;
        let v2 = (v as f32 * (0.85 + 0.15 * (t * 0.5).sin())) as i16;
        out.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(&v2.to_le_bytes());
    }
    let tmp = path.with_extension("part");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&out)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Invents plausible tracks with evidence pointing at an unroutable
/// example domain, so demo data can never be mistaken for real findings.
pub struct DemoSource;

const ARTISTS: &[&str] = &[
    "Fixture Collective",
    "Sine Wave Society",
    "The Placeholders",
    "Mock Unit",
    "Null Device",
    "Synthetic Sound System",
    "Test Pattern",
    "Lorem Ipsum Orchestra",
    "Dry Run",
    "Stub & Spy",
];
const WORDS: &[&str] = &[
    "Night", "Signal", "Tone", "Pulse", "Drift", "Circuit", "Echo", "Static", "Orbit", "Phase", "Groove",
    "Shadow", "Carrier", "Delay", "Harmonic", "Loop", "Vector", "Filter", "Horizon", "Chord",
];
const MIXES: &[Option<&str>] = &[
    None,
    Some("Original Mix"),
    Some("Dub"),
    Some("Extended Mix"),
    Some("Edit"),
];
const LABELS: &[&str] = &[
    "Test Pressings",
    "Oscillator Records",
    "Ipsum Audio",
    "Seeded Sounds",
    "Kernel Cuts",
];

impl DiscoverySource for DemoSource {
    fn id(&self) -> &str {
        "demo"
    }

    fn discover(&self, request: &DiscoveryRequest) -> AdapterResult<Vec<CandidateProposal>> {
        let limit = request.limit;
        Ok((0..limit)
            .map(|_| {
                let n = seed_of(&crate::util::new_id());
                let pick = |list: &[&'static str], shift: u32| list[((n >> shift) as usize) % list.len()];
                let artist = pick(ARTISTS, 0);
                let title = format!("{} {}", pick(WORDS, 8), pick(WORDS, 16));
                let mix = MIXES[((n >> 24) as usize) % MIXES.len()];
                let label = pick(LABELS, 32);
                CandidateProposal {
                    artist: artist.to_string(),
                    title: title.clone(),
                    mix: mix.map(str::to_string),
                    label: Some(label.to_string()),
                    release: None,
                    reasons: vec![format!("Demo: released on {label}")],
                    evidence: vec![EvidenceProposal {
                        source_kind: "demo".into(),
                        source_url: Some(format!(
                            "https://example.invalid/demo/{}",
                            title.to_lowercase().replace(' ', "-")
                        )),
                        supplied_text_id: None,
                        excerpt: format!("{artist} - {title} (generated demo track)"),
                        confidence: 0.5 + ((n >> 40) % 50) as f64 / 100.0,
                    }],
                }
            })
            .collect())
    }
}

fn seed_of(s: &str) -> u64 {
    let h = blake3::hash(s.as_bytes());
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())
}

fn file_name(q: &str) -> String {
    let clean: String = q
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.wav", clean.trim())
}

/// Transfers are identified by their destination path, so their state
/// survives restarts the way a real download client's would.
pub struct DemoAcquirer {
    pub seconds: u32,
}

impl Default for DemoAcquirer {
    fn default() -> Self {
        DemoAcquirer { seconds: 20 }
    }
}

impl Acquirer for DemoAcquirer {
    fn id(&self) -> &str {
        "demo"
    }

    fn search(&self, query: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>> {
        let name = match &query.mix {
            Some(m) => format!("{} - {} ({m})", query.artist, query.title),
            None => format!("{} - {}", query.artist, query.title),
        };
        Ok(vec![SearchResult {
            result_id: name.clone(),
            filename: file_name(&name),
            size_bytes: 44 + 44_100 * 4 * self.seconds as u64,
            duration_ms: Some(self.seconds as u64 * 1000),
            format: Some("wav".into()),
            bitrate_kbps: Some(1411),
        }])
    }

    fn enqueue(
        &self,
        result: &SearchResult,
        _idempotency_key: &str,
        dest_dir: &Path,
    ) -> AdapterResult<String> {
        std::fs::create_dir_all(dest_dir).map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        let dest = dest_dir.join(&result.filename);
        if !dest.exists() {
            write_tone_wav(&dest, seed_of(&result.result_id), self.seconds)
                .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        }
        Ok(dest.to_string_lossy().into_owned())
    }

    fn status(&self, transfer_id: &str) -> AdapterResult<TransferStatus> {
        let path = PathBuf::from(transfer_id);
        Ok(if path.exists() {
            TransferStatus::Completed { path }
        } else {
            TransferStatus::Failed {
                reason: "the generated file is no longer in staging".into(),
            }
        })
    }

    fn cancel(&self, _transfer_id: &str) -> AdapterResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_wav_is_valid_and_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        write_tone_wav(&a, 7, 1).unwrap();
        write_tone_wav(&b, 7, 1).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let info = cd_audio::probe(&a).unwrap();
        assert_eq!((info.sample_rate, info.channels), (44_100, 2));
        assert_eq!(cd_audio::decode::decode_all(&a).unwrap().decoded_ms, 1000);
    }

    #[test]
    fn enqueue_is_idempotent_and_status_survives_a_new_instance() {
        let dir = tempfile::tempdir().unwrap();
        let acq = DemoAcquirer { seconds: 1 };
        let q = AcquisitionQuery {
            artist: "A".into(),
            title: "B/C".into(),
            mix: Some("Dub".into()),
        };
        let r = acq.search(&q).unwrap().remove(0);
        let t1 = acq.enqueue(&r, "k", dir.path()).unwrap();
        let t2 = acq.enqueue(&r, "k", dir.path()).unwrap();
        assert_eq!(t1, t2);
        assert!(t1.ends_with("A - B_C (Dub).wav"));
        let fresh = DemoAcquirer { seconds: 1 };
        assert!(matches!(
            fresh.status(&t1).unwrap(),
            TransferStatus::Completed { .. }
        ));
    }
}
