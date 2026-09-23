//! Fingerprint comparison on real encoded files and synthetic audio.

use std::path::PathBuf;

use cd_audio::synth::{self, Song};
use cd_core::analysis::protocol::FingerprintOut;
use cd_core::identity::fingerprint::{compare, implied_speeds};

fn out(f: cd_audio::fingerprint::Fingerprint) -> FingerprintOut {
    FingerprintOut {
        algorithm: f.algorithm,
        data: f.data,
        duration_ms: f.duration_ms,
        speed: f.speed,
    }
}

fn samples(seed: u64, secs: f32) -> FingerprintOut {
    let a = Song::from_seed(seed).render(secs);
    out(cd_audio::fingerprint::fingerprint_samples(
        &a.samples,
        synth::RATE,
        2,
        1.0,
    ))
}

#[test]
fn a_file_matches_its_source_audio() {
    let a = samples(1, 30.0);
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures/identity/song1-128.mp3");
    let b = out(cd_audio::fingerprint::fingerprint_file(&path, 1.0).unwrap());
    let c = compare(&a, &b);
    assert!(c.score > 0.9 && c.coverage > 0.95, "{c:?}");
}

#[test]
fn unrelated_songs_do_not_match() {
    let c = compare(&samples(1, 30.0), &samples(2, 30.0));
    assert!(c.score * c.coverage < 0.2, "{c:?}");
}

#[test]
fn different_algorithms_are_never_compared() {
    let a = samples(3, 10.0);
    let mut b = a.clone();
    b.algorithm = "something-else".into();
    assert_eq!(compare(&a, &b).coverage, 0.0);
}

#[test]
fn speeds_come_from_the_length_ratio_within_a_dj_range() {
    assert!(implied_speeds(400_000, 400_500).is_empty(), "too close to 1");
    assert!(implied_speeds(400_000, 300_000).is_empty(), "beyond 12%");
    let s = implied_speeds(416_000, 400_000);
    assert!((s[0] - 1.04).abs() < 1e-9);
}
