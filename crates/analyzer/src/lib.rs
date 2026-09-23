//! Crate Digger's audio analysis, run in its own process (`cd-analyzer`).
//!
//! One pass over the decoded file feeds every feature: the fingerprint,
//! loudness, quality counters and short-time spectral frames. Tempo, key
//! and embeddings are computed from the frames afterwards, so memory use
//! stays small even for long mixes.

pub mod embed;
pub mod frames;
pub mod key;
pub mod mel16k;
pub mod models;
pub mod tempo;

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cd_audio::decode::{AudioError, Decoder};
use cd_audio::fingerprint;
use cd_core::analysis::protocol::{
    Analysis, EmbeddingOut, ErrorKind, FingerprintOut, Message, Quality, Request, Segment, Stats,
};

/// Length of each analysed segment.
pub const SEGMENT_MS: u64 = 30_000;
/// Width of the stored waveform overview.
pub const WAVEFORM_BINS: usize = 800;

/// Peak resident memory of this process in KiB.
pub fn peak_rss_kb() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: getrusage fills the struct we pass it.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
            return 0;
        }
        let max = usage.ru_maxrss as u64;
        // macOS reports bytes, Linux kibibytes.
        if cfg!(target_vendor = "apple") {
            max / 1024
        } else {
            max
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: the counters struct is sized and zeroed before the call.
        unsafe {
            let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
                return (c.PeakWorkingSetSize / 1024) as u64;
            }
        }
        0
    }
    #[cfg(not(any(unix, windows)))]
    {
        0
    }
}

pub fn error_kind(e: &AudioError) -> ErrorKind {
    match e {
        AudioError::NotFound(_) => ErrorKind::NotFound,
        AudioError::Unreadable { .. } => ErrorKind::Unreadable,
        AudioError::Unsupported(_) => ErrorKind::Unsupported,
        AudioError::Corrupt(_) => ErrorKind::Corrupt,
    }
}

/// Up to three 30-second windows centred at 25, 50 and 75% of the track;
/// the whole track when it is too short for that.
pub fn segments(duration_ms: u64) -> Vec<Segment> {
    if duration_ms < SEGMENT_MS * 3 {
        return vec![Segment {
            start_ms: 0,
            end_ms: duration_ms,
        }];
    }
    [0.25, 0.5, 0.75]
        .iter()
        .map(|p| {
            let centre = (duration_ms as f64 * p) as u64;
            let start = centre
                .saturating_sub(SEGMENT_MS / 2)
                .min(duration_ms - SEGMENT_MS);
            Segment {
                start_ms: start,
                end_ms: start + SEGMENT_MS,
            }
        })
        .collect()
}

pub fn fingerprint_out(fp: fingerprint::Fingerprint) -> FingerprintOut {
    FingerprintOut {
        algorithm: fp.algorithm,
        data: fp.data,
        duration_ms: fp.duration_ms,
        speed: fp.speed,
    }
}

/// Analyse a file. `models` add pretrained embeddings to the built-in
/// baseline. `progress` receives the decoded position in ms.
pub fn analyse(
    path: &Path,
    models: &[models::LoadedModel],
    progress: &dyn Fn(u64),
) -> Result<Analysis, AudioError> {
    let started = Instant::now();
    let mut dec = Decoder::open(path)?;
    let (rate, channels) = (dec.info.sample_rate, dec.info.channels.max(1));
    let header_ms = dec.info.duration_ms;
    let mut fp = fingerprint::Streaming::new(rate, channels, 1.0);
    let mut loudness = ebur128::EbuR128::new(channels as u32, rate, ebur128::Mode::I).ok();
    let mut frames = frames::FrameAnalyzer::new(rate);
    let (mut samples, mut clipped) = (0u64, 0u64);
    let mut mono = Vec::new();
    let mut buf = Vec::new();
    let mut last_report = 0u64;
    let (mut blocks, mut block_peak, mut in_block) = (Vec::new(), 0f32, 0usize);
    // Pretrained models need 16 kHz mel frames; only computed when used.
    let mut to_16k = (!models.is_empty()).then(|| cd_audio::resample::MonoResampler::new(rate, mel16k::RATE));
    let mut mel = mel16k::MelFrames::new();
    let mut mono16k = Vec::new();

    while dec.next_chunk(&mut buf)? {
        let ch = dec.info.channels.max(1) as usize;
        fp.consume(&buf);
        if let Some(l) = loudness.as_mut() {
            if l.add_frames_f32(&buf).is_err() {
                loudness = None;
            }
        }
        samples += buf.len() as u64;
        clipped += buf.iter().filter(|s| s.abs() >= 0.999).count() as u64;
        mono.clear();
        mono.extend(buf.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
        for frame in buf.chunks_exact(ch) {
            block_peak = frame.iter().fold(block_peak, |m, s| m.max(s.abs()));
            in_block += 1;
            if in_block == cd_audio::waveform::BLOCK {
                blocks.push(block_peak);
                (block_peak, in_block) = (0.0, 0);
            }
        }
        frames.push(&mono);
        if let Some(r) = to_16k.as_mut() {
            mono16k.clear();
            r.process(&mono, &mut mono16k);
            mel.push(&mono16k);
        }
        let pos = dec.position_ms();
        if pos >= last_report + 1000 {
            last_report = pos;
            progress(pos);
        }
    }
    if in_block > 0 {
        blocks.push(block_peak);
    }
    if let Some(r) = to_16k.as_mut() {
        mono16k.clear();
        r.finish(&mut mono16k);
        mel.push(&mono16k);
    }
    let frame_count = samples / channels as u64;
    if frame_count == 0 {
        return Err(AudioError::Corrupt("no audio frames could be decoded".into()));
    }
    let duration_ms = frame_count * 1000 / rate as u64;
    let fps = frames.frames_per_second();
    let all = &frames.frames;

    let segs = segments(duration_ms);
    let version = embed::version();
    let mut vectors = Vec::new();
    let mut embeddings = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let a = ((s.start_ms as f32 / 1000.0) * fps) as usize;
        let b = (((s.end_ms as f32 / 1000.0) * fps) as usize).min(all.len());
        if a >= b {
            continue;
        }
        let v = embed::embed(&all[a..b]);
        vectors.push(v.clone());
        embeddings.push(EmbeddingOut {
            version: version.clone(),
            segment: Some(i),
            vector: v,
        });
    }
    if !vectors.is_empty() {
        embeddings.push(EmbeddingOut {
            version,
            segment: None,
            vector: embed::mean(&vectors),
        });
    }
    let mel_fps = mel16k::MelFrames::frames_per_second();
    for m in models {
        let mut model_vectors = Vec::new();
        for (i, s) in segs.iter().enumerate() {
            let a = ((s.start_ms as f32 / 1000.0) * mel_fps) as usize;
            let b = (((s.end_ms as f32 / 1000.0) * mel_fps) as usize).min(mel.frames.len());
            if a >= b {
                continue;
            }
            match m.embed(&mel.frames[a..b]) {
                Ok(Some(v)) => {
                    model_vectors.push(v.clone());
                    embeddings.push(EmbeddingOut {
                        version: m.version(),
                        segment: Some(i),
                        vector: v,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(AudioError::Unsupported(format!(
                        "model {} failed: {e}",
                        m.info.id
                    )))
                }
            }
        }
        if !model_vectors.is_empty() {
            let n = model_vectors.len() as f32;
            let mut mean = vec![0f32; model_vectors[0].len()];
            for v in &model_vectors {
                for (a, b) in mean.iter_mut().zip(v) {
                    *a += b / n;
                }
            }
            embeddings.push(EmbeddingOut {
                version: m.version(),
                segment: None,
                vector: mean,
            });
        }
    }

    let (tempo_bpm, tempo_confidence) = match tempo::estimate(all, fps) {
        Some((b, c)) => (Some(b), c),
        None => (None, 0.0),
    };
    let silent = all.iter().filter(|f| f.rms < 1e-3).count();
    Ok(Analysis {
        duration_ms,
        sample_rate: rate,
        channels,
        fingerprint: fingerprint_out(fp.finish()),
        tempo_bpm,
        tempo_confidence,
        key: key::estimate(&frames.chroma_frames),
        loudness_lufs: loudness
            .and_then(|l| l.loudness_global().ok())
            .filter(|v| v.is_finite()),
        quality: Quality {
            decoded_fraction: header_ms.map(|h| duration_ms as f32 / h.max(1) as f32),
            clipping_ratio: clipped as f32 / samples.max(1) as f32,
            silence_ratio: silent as f32 / all.len().max(1) as f32,
            decode_errors: dec.decode_errors,
        },
        segments: segs,
        embeddings,
        waveform: cd_audio::waveform::bin_peaks(&blocks, WAVEFORM_BINS),
        stats: Stats {
            wall_ms: started.elapsed().as_millis() as u64,
            peak_rss_kb: peak_rss_kb(),
        },
    })
}

// ---------------------------------------------------------------------------
// Worker process
// ---------------------------------------------------------------------------

/// Command-line flag that makes the app binary act as the worker.
pub const WORKER_FLAG: &str = "--analysis-worker";

fn send(msg: &Message) {
    let line = serde_json::to_string(msg).expect("messages serialise");
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The worker's entry point: one request on stdin, heartbeats and one
/// final message on stdout. Used by the `cd-analyzer` binary and by the app
/// when started with `--analysis-worker`.
pub fn worker_main() {
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() || line.trim().is_empty() {
        send(&Message::Error {
            kind: ErrorKind::Internal,
            message: "no request received".into(),
        });
        return;
    }
    let fault = std::env::var("CD_ANALYZER_FAULT").unwrap_or_default();
    match fault.as_str() {
        "crash" => std::process::abort(),
        "garbage" => {
            println!("this is not json");
            std::thread::sleep(Duration::from_secs(5));
            return;
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        _ => {}
    }

    let request: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            send(&Message::Error {
                kind: ErrorKind::Internal,
                message: format!("bad request: {e}"),
            });
            return;
        }
    };

    let decoded = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let heartbeat = {
        let (decoded, done) = (decoded.clone(), done.clone());
        let fake_memory = fault == "memory";
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                send(&Message::Heartbeat {
                    decoded_ms: decoded.load(Ordering::Relaxed),
                    peak_rss_kb: if fake_memory { u64::MAX / 2 } else { peak_rss_kb() },
                });
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    };

    let result = match request {
        Request::Analyse { path, models: refs } => {
            let mut loaded = Vec::new();
            for r in &refs {
                match models::load(r) {
                    Ok(m) => loaded.push(m),
                    Err(message) => {
                        done.store(true, Ordering::Relaxed);
                        let _ = heartbeat.join();
                        send(&Message::Error {
                            kind: ErrorKind::Internal,
                            message,
                        });
                        return;
                    }
                }
            }
            analyse(Path::new(&path), &loaded, &|ms| {
                decoded.store(ms, Ordering::Relaxed)
            })
            .map(|a| Message::Analysis(Box::new(a)))
        }
        Request::Fingerprint { path, speed } => {
            cd_audio::fingerprint::fingerprint_file(Path::new(&path), speed)
                .map(|f| Message::Fingerprint(fingerprint_out(f)))
        }
    };
    done.store(true, Ordering::Relaxed);
    let _ = heartbeat.join();
    match result {
        Ok(m) => send(&m),
        Err(e) => send(&Message::Error {
            kind: error_kind(&e),
            message: e.user_message(),
        }),
    }
}
