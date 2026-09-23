//! Playback through the null output, which consumes audio at real-time
//! pace. These run on CI machines with no sound device.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cd_audio::{AudioError, OutputConfig, PlayState, Player};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn null_player(rate: u32) -> Player {
    Player::start(OutputConfig::Null {
        sample_rate: rate,
        channels: 2,
        period_frames: 512,
    })
    .unwrap()
}

fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn plays_pauses_and_reports_position() {
    let p = null_player(48_000);
    assert_eq!(p.status().state, PlayState::Idle);
    let info = p.load(&fixture("tone.flac"), 0, true).unwrap();
    assert_eq!(info.sample_rate, 44_100);
    assert!(wait_until(1000, || p.status().position_ms >= 300));
    assert_eq!(p.status().state, PlayState::Playing);

    p.pause();
    std::thread::sleep(Duration::from_millis(50));
    let paused_at = p.status().position_ms;
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(p.status().state, PlayState::Paused);
    assert!(
        p.status().position_ms <= paused_at + 15,
        "position moved while paused"
    );

    p.play();
    assert!(wait_until(1000, || p.status().position_ms > paused_at + 100));
}

#[test]
fn seeking_moves_the_position() {
    let p = null_player(44_100);
    p.load(&fixture("tone.wav"), 0, true).unwrap();
    p.seek(1_500);
    assert!(wait_until(500, || {
        let pos = p.status().position_ms;
        (1_500..1_800).contains(&pos)
    }));
}

#[test]
fn starts_from_an_offset() {
    let p = null_player(48_000);
    p.load(&fixture("tone.mp3"), 1_000, true).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(p.status().position_ms >= 1_000);
}

#[test]
fn reaches_the_end_and_can_restart() {
    let p = null_player(48_000);
    p.load(&fixture("tone.ogg"), 1_700, true).unwrap();
    assert!(
        wait_until(2_000, || p.status().state == PlayState::Ended),
        "{:?}",
        p.status()
    );
    assert!(!p.is_playing());
    p.play();
    assert!(wait_until(5_000, || p.status().state == PlayState::Playing
        && p.status().position_ms < 1_000));
}

#[test]
fn missing_and_corrupt_files_fail_to_load_without_disturbing_playback() {
    let p = null_player(48_000);
    p.load(&fixture("tone.flac"), 0, true).unwrap();
    assert!(wait_until(5_000, || p.status().state == PlayState::Playing));
    assert!(matches!(
        p.load(&fixture("nope.flac"), 0, true),
        Err(AudioError::NotFound(_))
    ));
    assert!(p.load(&fixture("corrupt.mp3"), 0, true).is_err());
    // The track that was playing keeps playing.
    assert_eq!(p.status().state, PlayState::Playing);
    assert_eq!(p.status().path.unwrap(), fixture("tone.flac"));
}

#[test]
fn volume_is_clamped() {
    let p = null_player(48_000);
    p.set_volume(3.0);
    assert_eq!(p.status().volume, 1.0);
    p.set_volume(-1.0);
    assert_eq!(p.status().volume, 0.0);
}

#[test]
fn stop_returns_to_idle() {
    let p = null_player(48_000);
    p.load(&fixture("tone.flac"), 0, true).unwrap();
    p.stop();
    assert!(wait_until(500, || p.status().state == PlayState::Idle));
}

/// Acceptance: playback stays smooth while analysis-like work runs. The
/// scheduler allows one analysis job (plus import) at a time, so two busy
/// decoding threads represent the worst normal background load.
#[test]
fn no_underruns_while_background_decoding_runs() {
    let stop = Arc::new(AtomicBool::new(false));
    let load: Vec<_> = (0..2)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = cd_audio::decode::decode_all(&fixture("tone.flac"));
                    let _ = cd_audio::waveform::peaks(&fixture("other-stereo.flac"), 800);
                }
            })
        })
        .collect();

    let p = null_player(48_000);
    // Resampling 44.1 kHz to 48 kHz exercises the whole decode path.
    p.load(&fixture("tone.flac"), 0, true).unwrap();
    // On a loaded CI machine the null output's clock can itself run late,
    // which is not an underrun; allow plenty of time to reach the end.
    assert!(wait_until(20_000, || p.status().state == PlayState::Ended));
    stop.store(true, Ordering::Relaxed);
    for t in load {
        t.join().unwrap();
    }
    let s = p.status();
    eprintln!("underruns under load: {}", s.underruns);
    assert_eq!(s.underruns, 0);
}
