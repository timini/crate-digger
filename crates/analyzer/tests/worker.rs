//! The worker as a separate process: results, failures and isolation.

use std::path::PathBuf;
use std::time::Duration;

use cd_core::analysis::protocol::{ErrorKind, Message, Request};
use cd_core::analysis::runner::{run, RunnerConfig, WorkerError};

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cd-analyzer"))
}

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../audio/tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn cfg() -> RunnerConfig {
    let mut c = RunnerConfig::new(exe());
    c.heartbeat_timeout = Duration::from_secs(5);
    c.timeout = Duration::from_secs(60);
    c
}

fn with_fault(fault: &str) -> RunnerConfig {
    let mut c = cfg();
    c.env.push(("CD_ANALYZER_FAULT".into(), fault.into()));
    c.heartbeat_timeout = Duration::from_secs(2);
    c
}

fn analyse(path: &str) -> Request {
    Request::Analyse {
        path: path.into(),
        models: vec![],
    }
}

#[test]
fn analyses_a_file_through_the_worker() {
    let msg = run(&cfg(), &analyse(&fixture("identity/song1-128.mp3"))).unwrap();
    let Message::Analysis(a) = msg else {
        panic!("{msg:?}")
    };
    assert!((29_500..=30_500).contains(&a.duration_ms), "{}", a.duration_ms);
    assert!(!a.fingerprint.data.is_empty());
    assert!(a.stats.peak_rss_kb > 0);
    assert!(a.tempo_bpm.is_some());
}

#[test]
fn fingerprint_request_applies_speed() {
    let msg = run(
        &cfg(),
        &Request::Fingerprint {
            path: fixture("tone.flac"),
            speed: 1.04,
        },
    )
    .unwrap();
    let Message::Fingerprint(f) = msg else {
        panic!("{msg:?}")
    };
    assert_eq!(f.speed, 1.04);
    assert!((2070..=2090).contains(&f.duration_ms), "{}", f.duration_ms);
}

#[test]
fn missing_and_corrupt_files_are_reported_not_retried() {
    let err = run(&cfg(), &analyse(&fixture("nope.flac"))).unwrap_err();
    assert!(
        matches!(
            err,
            WorkerError::Failed {
                kind: ErrorKind::NotFound,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!err.retryable());
    let err = run(&cfg(), &analyse(&fixture("truncated.flac"))).unwrap_err();
    assert!(
        matches!(
            err,
            WorkerError::Failed {
                kind: ErrorKind::Corrupt | ErrorKind::Unsupported,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("damaged") || err.to_string().contains("not supported"),
        "{err}"
    );
}

#[test]
fn a_crash_is_contained() {
    let err = run(&with_fault("crash"), &analyse(&fixture("tone.flac"))).unwrap_err();
    assert!(matches!(err, WorkerError::Crashed { .. }), "{err:?}");
    assert!(err.retryable());
}

#[test]
fn a_hang_is_detected_by_missing_heartbeats() {
    let started = std::time::Instant::now();
    let err = run(&with_fault("hang"), &analyse(&fixture("tone.flac"))).unwrap_err();
    assert_eq!(err, WorkerError::Unresponsive);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn garbage_output_is_rejected() {
    let err = run(&with_fault("garbage"), &analyse(&fixture("tone.flac"))).unwrap_err();
    assert!(matches!(err, WorkerError::BadOutput(_)), "{err:?}");
}

#[test]
fn runaway_memory_is_stopped() {
    let err = run(&with_fault("memory"), &analyse(&fixture("tone.flac"))).unwrap_err();
    assert!(matches!(err, WorkerError::OverMemory { .. }), "{err:?}");
}

#[test]
fn overall_timeout_applies() {
    let mut c = cfg();
    c.timeout = Duration::from_millis(1);
    let err = run(&c, &analyse(&fixture("identity/song1-128.mp3"))).unwrap_err();
    assert!(matches!(err, WorkerError::TimedOut(_)), "{err:?}");
}

#[test]
fn missing_worker_binary_is_a_clear_error() {
    let err = run(
        &RunnerConfig::new("/nonexistent/cd-analyzer".into()),
        &analyse("x"),
    )
    .unwrap_err();
    assert!(matches!(err, WorkerError::Spawn(_)));
}

/// Acceptance: an analysis failure leaves playback running.
#[test]
fn playback_continues_while_the_worker_crashes() {
    let player = cd_audio::Player::start(cd_audio::OutputConfig::Null {
        sample_rate: 48_000,
        channels: 2,
        period_frames: 512,
    })
    .unwrap();
    player
        .load(std::path::Path::new(&fixture("tone.flac")), 0, true)
        .unwrap();
    for fault in ["crash", "garbage", "memory"] {
        assert!(run(&with_fault(fault), &analyse(&fixture("tone.flac"))).is_err());
    }
    let s = player.status();
    assert!(
        matches!(s.state, cd_audio::PlayState::Playing | cd_audio::PlayState::Ended),
        "{s:?}"
    );
    assert_eq!(s.underruns, 0);
    assert!(s.error.is_none());
}
