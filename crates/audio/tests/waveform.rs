use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn constant_tone_has_a_flat_waveform() {
    let peaks = cd_audio::waveform::peaks(&fixture("tone.flac"), 200).unwrap();
    assert_eq!(peaks.len(), 200);
    // ffmpeg's sine source is at 1/8 full scale.
    let (min, max) = peaks
        .iter()
        .fold((255u8, 0u8), |(a, b), p| (a.min(*p), b.max(*p)));
    assert!(max - min <= 3, "min {min} max {max}");
    assert!(max > 20);
}

#[test]
fn corrupt_file_has_no_waveform() {
    assert!(cd_audio::waveform::peaks(&fixture("truncated.flac"), 100).is_err());
}
