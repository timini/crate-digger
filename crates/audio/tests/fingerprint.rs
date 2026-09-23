use std::path::PathBuf;

use cd_audio::fingerprint::{compare, fingerprint_file, fingerprint_samples, Fingerprint};
use cd_audio::synth::{self, Song};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/identity")
        .join(name)
}

#[test]
fn a_file_matches_its_source_audio() {
    let song = Song::from_seed(1).render(30.0);
    let a = fingerprint_samples(&song.samples, synth::RATE, 2, 1.0);
    let b = fingerprint_file(&fixture("song1-128.mp3"), 1.0).unwrap();
    let c = compare(&a, &b);
    assert!(c.score > 0.9 && c.coverage > 0.95, "{c:?}");
    assert!((b.duration_ms as i64 - 30_000).abs() < 200, "{}", b.duration_ms);
}

#[test]
fn storage_round_trip_keeps_the_data() {
    let fp = fingerprint_samples(&Song::from_seed(2).render(10.0).samples, synth::RATE, 2, 1.0);
    let back = Fingerprint::from_bytes(&fp.to_bytes(), fp.duration_ms);
    assert_eq!(back.data, fp.data);
    assert_eq!(compare(&fp, &back).score, 1.0);
}

#[test]
fn different_algorithms_are_never_compared() {
    let fp = fingerprint_samples(&Song::from_seed(3).render(10.0).samples, synth::RATE, 2, 1.0);
    let mut other = fp.clone();
    other.algorithm = "something-else".into();
    let c = compare(&fp, &other);
    assert_eq!((c.score, c.coverage), (0.0, 0.0));
}

#[test]
fn speed_compensation_restores_the_duration() {
    let orig = Song::from_seed(4).render(20.0);
    let fast = synth::speed(&orig, 1.05);
    let fp = fingerprint_samples(&fast.samples, synth::RATE, 2, 1.05);
    assert!((fp.duration_ms as i64 - 20_000).abs() < 50, "{}", fp.duration_ms);
}
