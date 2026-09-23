//! Benchmark analysis cost through the real worker process, one file at a
//! time, so time and peak memory are measured per process.
//!
//! cargo build --release -p cd-analyzer
//! cargo run --release -p cd-analyzer --example bench -- <music folder> <models folder> [files]
//!
//! Prints per-backend time per minute of audio and peak memory, as
//! aggregates only.

use std::path::{Path, PathBuf};

use cd_analyzer::models::REGISTRY;
use cd_core::analysis::protocol::{Message, ModelRef, Request};
use cd_core::analysis::runner::{run, RunnerConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let music = PathBuf::from(
        args.next()
            .expect("usage: bench <music folder> <models folder> [files]"),
    );
    let models_dir = PathBuf::from(args.next().expect("models folder"));
    let limit: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);

    let exe = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(Path::parent)
        .map(|d| {
            d.join(if cfg!(windows) {
                "cd-analyzer.exe"
            } else {
                "cd-analyzer"
            })
        })
        .expect("worker binary next to the examples folder");
    assert!(
        exe.exists(),
        "build the worker first: cargo build --release -p cd-analyzer"
    );

    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&music)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && cd_audio::decode::is_supported_extension(e.path()))
        .map(|e| e.into_path())
        .collect();
    files.sort_by_key(|p| blake3::hash(p.to_string_lossy().as_bytes()).to_hex().to_string());
    files.truncate(limit);

    let mut backends: Vec<(String, Vec<ModelRef>)> = vec![("cd-dsp-v1 only".into(), vec![])];
    for m in REGISTRY {
        let path = models_dir.join(format!("{}.onnx", m.id));
        if path.exists() {
            backends.push((
                format!("cd-dsp-v1 + {}", m.id),
                vec![ModelRef {
                    id: m.id.into(),
                    path: path.to_string_lossy().into(),
                    sha256: m.sha256.into(),
                }],
            ));
        }
    }

    println!("| Backends | Files | Audio minutes | Seconds per audio minute | Median seconds per track | Peak memory (MB) |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (name, models) in backends {
        let cfg = RunnerConfig::new(exe.clone());
        let (mut wall_ms, mut audio_ms, mut peak_kb, mut per_track) = (0u64, 0u64, 0u64, Vec::new());
        for f in &files {
            let req = Request::Analyse {
                path: f.to_string_lossy().into(),
                models: models.clone(),
            };
            if let Ok(Message::Analysis(a)) = run(&cfg, &req) {
                wall_ms += a.stats.wall_ms;
                audio_ms += a.duration_ms;
                peak_kb = peak_kb.max(a.stats.peak_rss_kb);
                per_track.push(a.stats.wall_ms);
            }
        }
        per_track.sort();
        let median = per_track.get(per_track.len() / 2).copied().unwrap_or(0);
        println!(
            "| {name} | {} | {:.1} | {:.2} | {:.2} | {} |",
            per_track.len(),
            audio_ms as f64 / 60_000.0,
            (wall_ms as f64 / 1000.0) / (audio_ms as f64 / 60_000.0).max(1e-9),
            median as f64 / 1000.0,
            peak_kb / 1024
        );
    }
}
