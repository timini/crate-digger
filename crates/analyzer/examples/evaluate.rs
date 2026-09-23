//! Evaluate embedding backends on a real music library.
//!
//! cargo run --release -p cd-analyzer --example evaluate -- <music folder> <models folder> [tracks] [out.md]
//!
//! Samples up to `tracks` files (default 150), crops each to its middle two
//! minutes, and derives variants: a remaster (EQ, level, saturation),
//! copies played 4% faster and slower, and an MP3 re-encode if ffmpeg is
//! available. For every backend (the built-in cd-dsp-v1 and each model
//! found in the models folder) it measures how well same-recording pairs
//! separate from different tracks, how consistent different sections of one
//! track are, how different mixes of one title compare, and how estimated
//! tempo and key agree with the files' tags.
//!
//! Only aggregate numbers are written. No paths, names or audio leave the
//! machine, and nothing in the music folder is modified.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cd_analyzer::models::{self, LoadedModel, REGISTRY};
use cd_audio::decode::Decoder;
use cd_core::analysis::protocol::{Analysis, ModelRef};
use cd_core::identity::normalize::{classify_mix, parse_artists, title_key};

const CROP_SECONDS: f64 = 120.0;

struct Clip {
    rate: u32,
    /// Interleaved stereo.
    samples: Vec<f32>,
}

fn decode_middle(path: &Path) -> Option<Clip> {
    let mut dec = Decoder::open(path).ok()?;
    let rate = dec.info.sample_rate;
    let mut all = Vec::new();
    let mut buf = Vec::new();
    while dec.next_chunk(&mut buf).ok()? {
        let ch = dec.info.channels.max(1) as usize;
        for f in buf.chunks_exact(ch) {
            let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
            all.push(l);
            all.push(r);
        }
    }
    let frames = all.len() / 2;
    let want = (CROP_SECONDS * rate as f64) as usize;
    if frames < (100.0 * rate as f64) as usize {
        return None; // too short for three analysis segments
    }
    let start = frames.saturating_sub(want) / 2;
    let end = (start + want).min(frames);
    Some(Clip {
        rate,
        samples: all[start * 2..end * 2].to_vec(),
    })
}

fn write_wav(path: &Path, clip: &Clip) {
    let data_len = (clip.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&clip.rate.to_le_bytes());
    out.extend_from_slice(&(clip.rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &clip.samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    std::fs::write(path, out).expect("write temporary wav");
}

fn variant(clip: &Clip, f: impl Fn(&cd_audio::synth::Audio) -> cd_audio::synth::Audio) -> Clip {
    let a = cd_audio::synth::Audio {
        samples: clip.samples.clone(),
    };
    Clip {
        rate: clip.rate,
        samples: f(&a).samples,
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut d, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        d += *x as f64 * *y as f64;
        na += (*x as f64).powi(2);
        nb += (*y as f64).powi(2);
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        d / (na.sqrt() * nb.sqrt())
    }
}

/// Probability that a random positive scores above a random negative.
fn auc(pos: &[f64], neg: &[f64]) -> Option<f64> {
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut wins = 0.0;
    for p in pos {
        for n in neg {
            wins += if p > n {
                1.0
            } else if p == n {
                0.5
            } else {
                0.0
            };
        }
    }
    Some(wins / (pos.len() * neg.len()) as f64)
}

fn mean_sd(v: &[f64]) -> (f64, f64) {
    let n = v.len().max(1) as f64;
    let m = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n;
    (m, var.sqrt())
}

fn tpr_at_fpr(pos: &[f64], neg: &[f64], fpr: f64) -> Option<f64> {
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut n = neg.to_vec();
    n.sort_by(|a, b| a.total_cmp(b));
    let idx = (((1.0 - fpr) * n.len() as f64).ceil() as usize).min(n.len() - 1);
    let threshold = n[idx];
    Some(pos.iter().filter(|p| **p > threshold).count() as f64 / pos.len() as f64)
}

fn fmt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.3}")).unwrap_or_else(|| "-".into())
}

/// (pitch class, minor) from tags such as "8A", "Am", "A minor", "F#m", "1m".
fn parse_key(s: &str) -> Option<(usize, bool)> {
    let t = s.trim();
    let upper = t.to_uppercase();
    if let Some(num) = upper.strip_suffix('A').or_else(|| upper.strip_suffix('B')) {
        if let Ok(n) = num.trim().parse::<usize>() {
            if (1..=12).contains(&n) {
                let minor = upper.ends_with('A');
                // Camelot n: major root = (n - 8) * 7 mod 12 steps of fifths from C.
                let major_root = ((n + 12 - 8) * 7) % 12;
                return Some(if minor {
                    ((major_root + 9) % 12, true)
                } else {
                    (major_root, false)
                });
            }
        }
    }
    let names = [
        ("C", 0),
        ("B#", 0),
        ("C#", 1),
        ("DB", 1),
        ("D", 2),
        ("D#", 3),
        ("EB", 3),
        ("E", 4),
        ("FB", 4),
        ("F", 5),
        ("E#", 5),
        ("F#", 6),
        ("GB", 6),
        ("G", 7),
        ("G#", 8),
        ("AB", 8),
        ("A", 9),
        ("A#", 10),
        ("BB", 10),
        ("B", 11),
        ("CB", 11),
    ];
    let compact = upper.replace(' ', "").replace('♯', "#").replace('♭', "B");
    let (root, rest) = names
        .iter()
        .filter(|(n, _)| compact.starts_with(n))
        .max_by_key(|(n, _)| n.len())
        .map(|(n, pc)| (*pc, &compact[n.len()..]))?;
    let minor = rest.starts_with("M") && !rest.starts_with("MAJ") || rest.starts_with("MIN");
    let minor = minor || rest == "M";
    Some((root, minor))
}

/// MIREX key score: 1 exact, 0.5 fifth, 0.3 relative, 0.2 parallel.
fn key_score(est: (usize, bool), truth: (usize, bool)) -> f64 {
    if est == truth {
        1.0
    } else if est.1 == truth.1 && ((est.0 + 7) % 12 == truth.0 || (truth.0 + 7) % 12 == est.0) {
        0.5
    } else if est.1 != truth.1
        && ((!truth.1 && est.0 == (truth.0 + 9) % 12) || (truth.1 && est.0 == (truth.0 + 3) % 12))
    {
        0.3
    } else if est.0 == truth.0 {
        0.2
    } else {
        0.0
    }
}

struct TrackResult {
    artist: Vec<String>,
    title_key: Option<String>,
    mix: String,
    /// backend -> variant -> summary embedding
    embeddings: HashMap<String, HashMap<String, Vec<f32>>>,
    /// backend -> segment embeddings of the original
    segments: HashMap<String, Vec<Vec<f32>>>,
    tempo: Option<(f32, f32)>,
    key: Option<(String, String)>,
    ms: u64,
    peak_kb: u64,
}

fn collect(a: &Analysis, name: &str, r: &mut TrackResult) {
    for e in &a.embeddings {
        let backend = e.version.model_id.clone();
        match e.segment {
            None => {
                r.embeddings
                    .entry(backend)
                    .or_default()
                    .insert(name.to_string(), e.vector.clone());
            }
            Some(_) if name == "original" => {
                r.segments.entry(backend).or_default().push(e.vector.clone());
            }
            _ => {}
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let music = PathBuf::from(
        args.next()
            .expect("usage: evaluate <music folder> <models folder> [tracks] [out.md]"),
    );
    let models_dir = PathBuf::from(args.next().expect("models folder"));
    let limit: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(150);
    let out_path = args.next().map(PathBuf::from);
    let have_ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();

    let refs: Vec<ModelRef> = REGISTRY
        .iter()
        .map(|m| ModelRef {
            id: m.id.into(),
            path: models_dir.join(format!("{}.onnx", m.id)).to_string_lossy().into(),
            sha256: m.sha256.into(),
        })
        .filter(|r| Path::new(&r.path).exists())
        .collect();

    // Deterministic sample: sort by a hash of the path.
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&music)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && cd_audio::decode::is_supported_extension(e.path()))
        .map(|e| e.into_path())
        .collect();
    files.sort_by_key(|p| blake3::hash(p.to_string_lossy().as_bytes()).to_hex().to_string());
    eprintln!(
        "{} audio files found; evaluating up to {limit}; models: {}; ffmpeg: {have_ffmpeg}",
        files.len(),
        refs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")
    );

    let tmp = tempfile::tempdir().expect("temporary folder");
    let results: Arc<Mutex<Vec<TrackResult>>> = Arc::new(Mutex::new(Vec::new()));
    let queue = Arc::new(Mutex::new(files.into_iter()));
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let workers: Vec<_> = (0..threads)
        .map(|w| {
            let (queue, results, refs, done) = (queue.clone(), results.clone(), refs.clone(), done.clone());
            let dir = tmp.path().join(format!("w{w}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::thread::spawn(move || {
                let loaded: Vec<LoadedModel> = refs.iter().filter_map(|r| models::load(r).ok()).collect();
                loop {
                    if done.load(std::sync::atomic::Ordering::SeqCst) >= limit {
                        break;
                    }
                    let Some(path) = queue.lock().unwrap().next() else {
                        break;
                    };
                    let tags = cd_core::library::tags::read(&path).ok();
                    let Some(clip) = decode_middle(&path) else {
                        continue;
                    };
                    let started = Instant::now();
                    let mut r = TrackResult {
                        artist: tags
                            .as_ref()
                            .and_then(|t| t.artist.as_deref())
                            .map(|a| parse_artists(a).main)
                            .unwrap_or_default(),
                        title_key: tags.as_ref().and_then(|t| t.title.as_deref()).map(title_key),
                        mix: tags
                            .as_ref()
                            .map(|t| format!("{:?}", classify_mix(t.mix.as_deref())))
                            .unwrap_or_default(),
                        embeddings: HashMap::new(),
                        segments: HashMap::new(),
                        tempo: None,
                        key: None,
                        ms: 0,
                        peak_kb: 0,
                    };
                    let mut variants: Vec<(&str, Clip)> = vec![
                        ("remaster", variant(&clip, cd_audio::synth::remaster)),
                        ("faster4", variant(&clip, |a| cd_audio::synth::speed(a, 1.04))),
                        ("slower4", variant(&clip, |a| cd_audio::synth::speed(a, 0.96))),
                    ];
                    variants.insert(0, ("original", clip));
                    let mut ok = true;
                    for (name, c) in &variants {
                        let wav = dir.join(format!("{name}.wav"));
                        write_wav(&wav, c);
                        match cd_analyzer::analyse(&wav, &loaded, &|_| {}) {
                            Ok(a) => {
                                if *name == "original" {
                                    r.tempo = a
                                        .tempo_bpm
                                        .zip(tags.as_ref().and_then(|t| t.tempo).map(|t| t as f32));
                                    r.key = a
                                        .key
                                        .as_ref()
                                        .map(|k| k.name.clone())
                                        .zip(tags.as_ref().and_then(|t| t.musical_key.clone()));
                                    r.ms = started.elapsed().as_millis() as u64;
                                    r.peak_kb = a.stats.peak_rss_kb;
                                }
                                collect(&a, name, &mut r);
                            }
                            Err(_) => ok = false,
                        }
                    }
                    if have_ffmpeg && ok {
                        let wav = dir.join("original.wav");
                        let mp3 = dir.join("reencode.mp3");
                        let status = std::process::Command::new("ffmpeg")
                            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                            .arg(&wav)
                            .args(["-c:a", "libmp3lame", "-b:a", "128k"])
                            .arg(&mp3)
                            .status();
                        if status.map(|s| s.success()).unwrap_or(false) {
                            if let Ok(a) = cd_analyzer::analyse(&mp3, &loaded, &|_| {}) {
                                collect(&a, "reencode", &mut r);
                            }
                        }
                    }
                    if ok {
                        results.lock().unwrap().push(r);
                        let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        eprint!("\r{n} tracks analysed");
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    eprintln!();

    let results = results.lock().unwrap();
    let mut backends: Vec<String> = results
        .iter()
        .flat_map(|r| r.embeddings.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    backends.sort();

    let mut report = String::new();
    report.push_str(&format!(
        "# Embedding evaluation\n\n{} tracks (middle {} s of each), {} backends. Variants: remaster, ±4% speed{}.\n\n",
        results.len(),
        CROP_SECONDS,
        backends.len(),
        if have_ffmpeg { ", MP3 128 kbps re-encode" } else { "" }
    ));
    report.push_str("| Backend | Same recording vs random (AUC) | vs same artist (AUC) | TPR at 1% FPR | d′ | Pitch ±4% vs random (AUC) | Sections vs random (AUC) | Versions: mean sim | Versions vs random (AUC) | Same recording vs versions (AUC) |\n");
    report.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");

    let n = results.len();
    for b in &backends {
        let emb = |i: usize, v: &str| results[i].embeddings.get(b).and_then(|m| m.get(v));
        let (mut pos, mut pitch, mut random, mut artist, mut sections, mut section_neg, mut versions) =
            (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        for i in 0..n {
            let Some(o) = emb(i, "original") else { continue };
            for v in ["remaster", "reencode"] {
                if let Some(x) = emb(i, v) {
                    pos.push(cosine(o, x));
                }
            }
            for v in ["faster4", "slower4"] {
                if let Some(x) = emb(i, v) {
                    let s = cosine(o, x);
                    pos.push(s);
                    pitch.push(s);
                }
            }
            if let Some(segs) = results[i].segments.get(b) {
                if segs.len() >= 3 {
                    sections.push(cosine(&segs[0], &segs[2]));
                }
            }
            for k in 1..=5 {
                let j = (i * 7 + k * 13) % n;
                if j == i {
                    continue;
                }
                if let Some(x) = emb(j, "original") {
                    random.push(cosine(o, x));
                }
                if let (Some(a), Some(c)) = (results[i].segments.get(b), results[j].segments.get(b)) {
                    if !a.is_empty() && !c.is_empty() {
                        section_neg.push(cosine(&a[0], &c[c.len() - 1]));
                    }
                }
            }
            for j in (i + 1)..n {
                let same_artist = results[i].artist.iter().any(|a| results[j].artist.contains(a));
                if !same_artist {
                    continue;
                }
                let Some(x) = emb(j, "original") else { continue };
                let same_title =
                    results[i].title_key.is_some() && results[i].title_key == results[j].title_key;
                if same_title && results[i].mix != results[j].mix {
                    versions.push(cosine(o, x));
                } else if !same_title {
                    artist.push(cosine(o, x));
                }
            }
        }
        let (mp, sp) = mean_sd(&pos);
        let (mn, sn) = mean_sd(&random);
        let d_prime = (mp - mn) / (0.5 * (sp * sp + sn * sn)).sqrt().max(1e-9);
        report.push_str(&format!(
            "| {b} | {} | {} | {} | {d_prime:.2} | {} | {} | {} | {} | {} |\n",
            fmt(auc(&pos, &random)),
            fmt(auc(&pos, &artist)),
            fmt(tpr_at_fpr(&pos, &random, 0.01)),
            fmt(auc(&pitch, &random)),
            fmt(auc(&sections, &section_neg)),
            if versions.is_empty() {
                "-".into()
            } else {
                format!("{:.3} (n={})", mean_sd(&versions).0, versions.len())
            },
            fmt(auc(&versions, &random)),
            fmt(auc(&pos, &versions)),
        ));
    }

    // Tempo and key against tags.
    let tempo: Vec<(f32, f32)> = results.iter().filter_map(|r| r.tempo).collect();
    let within = |a: f32, b: f32| (a - b).abs() / b <= 0.02;
    let exact = tempo.iter().filter(|(e, t)| within(*e, *t)).count();
    let octave = tempo
        .iter()
        .filter(|(e, t)| {
            !within(*e, *t)
                && (within(*e * 2.0, *t)
                    || within(*e / 2.0, *t)
                    || within(*e * 1.5, *t)
                    || within(*e / 1.5, *t))
        })
        .count();
    let keys: Vec<f64> = results
        .iter()
        .filter_map(|r| r.key.as_ref())
        .filter_map(|(est, tag)| Some(key_score(parse_key(est)?, parse_key(tag)?)))
        .collect();
    let key_exact = keys.iter().filter(|s| **s == 1.0).count();
    let mirex = if keys.is_empty() {
        "-".to_string()
    } else {
        format!("{:.2}", keys.iter().sum::<f64>() / keys.len() as f64)
    };
    report.push_str(&format!(
        "\n## Tempo and key against tags\n\n- Tempo: {} tracks with a tagged BPM; {} within 2% ({:.0}%), {} off by a factor of 2 or 1.5.\n- Key: {} tracks with a tagged key; {} exact ({:.0}%), MIREX weighted score {}.\n",
        tempo.len(),
        exact,
        100.0 * exact as f64 / tempo.len().max(1) as f64,
        octave,
        keys.len(),
        key_exact,
        100.0 * key_exact as f64 / keys.len().max(1) as f64,
        mirex
    ));

    let (ms, _) = mean_sd(&results.iter().map(|r| r.ms as f64).collect::<Vec<_>>());
    let peak = results.iter().map(|r| r.peak_kb).max().unwrap_or(0);
    let mut by_backend = BTreeMap::new();
    for m in REGISTRY {
        by_backend.insert(m.id, m.licence);
    }
    report.push_str(&format!(
        "\n## Cost\n\nAnalysing one {CROP_SECONDS:.0} s clip with all backends took {ms:.0} ms on average ({threads} clips in parallel). Peak memory of the whole evaluation process: {} MB.\n",
        peak / 1024
    ));
    println!("{report}");
    if let Some(p) = out_path {
        std::fs::write(p, &report).expect("write report");
    }
}
