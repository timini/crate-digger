//! Measure playback smoothness on a real output device while background
//! decoding runs, for the reference-hardware report.
//!
//! cargo run --release -p cd-audio --example playback_harness -- <file> [seconds] [load_threads] [volume]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let file = PathBuf::from(
        args.next()
            .expect("usage: playback_harness <file> [seconds] [load_threads] [volume]"),
    );
    let seconds: u64 = args.next().map(|s| s.parse().unwrap()).unwrap_or(20);
    let threads: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(2);
    let volume: f32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(0.0);

    let stop = Arc::new(AtomicBool::new(false));
    let load: Vec<_> = (0..threads)
        .map(|_| {
            let stop = stop.clone();
            let file = file.clone();
            std::thread::spawn(move || {
                let mut passes = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let _ = cd_audio::waveform::peaks(&file, 1000);
                    passes += 1;
                }
                passes
            })
        })
        .collect();

    let player = cd_audio::Player::start(cd_audio::OutputConfig::Device).expect("output device");
    player.set_volume(volume);
    let info = player.load(&file, 0, true).expect("load");
    println!("output: {}", player.status().output);
    println!(
        "file: {} Hz, {} ch, {}",
        info.sample_rate, info.channels, info.codec
    );

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(500));
        let s = player.status();
        if s.state == cd_audio::PlayState::Ended {
            player.play();
        }
    }
    stop.store(true, Ordering::Relaxed);
    let passes: u64 = load.into_iter().map(|t| t.join().unwrap()).sum();
    let s = player.status();
    println!(
        "played {seconds} s with {threads} background decode threads ({passes} full decodes): {} underruns",
        s.underruns
    );
}
