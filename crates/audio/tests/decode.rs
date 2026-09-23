//! One decode test per advertised format, over committed synthetic
//! fixtures (two seconds of a 440 Hz tone; see scripts/gen-fixtures.sh).

use std::path::PathBuf;

use cd_audio::decode::{decode_all, is_supported_extension, probe, AudioError, SUPPORTED_FORMATS};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn assert_decodes(name: &str, codec: &str) {
    let path = fixture(name);
    let info = probe(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(info.codec, codec, "{name}");
    let report = decode_all(&path).unwrap();
    assert!(
        (1900..=2100).contains(&report.decoded_ms),
        "{name}: decoded {} ms, expected about 2000",
        report.decoded_ms
    );
    assert_eq!(report.decode_errors, 0, "{name}");
    if let Some(d) = info.duration_ms {
        assert!((1900..=2200).contains(&d), "{name}: header duration {d}");
    }
}

#[test]
fn decodes_wav() {
    assert_decodes("tone.wav", "pcm_s16le");
}

#[test]
fn decodes_aiff() {
    assert_decodes("tone.aiff", "pcm_s16be");
}

#[test]
fn decodes_flac() {
    assert_decodes("tone.flac", "flac");
}

#[test]
fn decodes_mp3() {
    assert_decodes("tone.mp3", "mp3");
    assert_decodes("tone-320.mp3", "mp3");
}

#[test]
fn decodes_aac_in_mp4() {
    assert_decodes("tone.m4a", "aac");
}

#[test]
fn decodes_alac_in_mp4() {
    assert_decodes("tone-alac.m4a", "alac");
}

#[test]
fn decodes_ogg_vorbis() {
    assert_decodes("tone.ogg", "vorbis");
}

#[test]
fn every_advertised_extension_has_a_fixture() {
    let tested = ["wav", "aiff", "flac", "mp3", "m4a", "ogg"];
    for (ext, _) in SUPPORTED_FORMATS {
        let base = match *ext {
            "aif" => "aiff",
            "mp4" => "m4a",
            e => e,
        };
        assert!(
            tested.contains(&base),
            "{ext} is advertised without a decode test"
        );
    }
}

#[test]
fn stereo_48k_reports_channels_and_rate() {
    let info = probe(&fixture("other-stereo.flac")).unwrap();
    assert_eq!((info.channels, info.sample_rate), (2, 48_000));
}

#[test]
fn missing_file_is_not_found() {
    let err = probe(&fixture("does-not-exist.flac")).unwrap_err();
    assert!(matches!(err, AudioError::NotFound(_)));
    assert!(err.user_message().contains("Relink"));
}

#[test]
fn random_bytes_are_rejected() {
    let err = probe(&fixture("corrupt.mp3")).unwrap_err();
    assert!(
        matches!(err, AudioError::Corrupt(_) | AudioError::Unsupported(_)),
        "{err:?}"
    );
}

#[test]
fn truncated_flac_is_corrupt() {
    let err = probe(&fixture("truncated.flac")).unwrap_err();
    assert!(
        matches!(err, AudioError::Corrupt(_) | AudioError::Unsupported(_)),
        "{err:?}"
    );
}

#[test]
fn seeking_lands_near_the_target() {
    let mut dec = cd_audio::Decoder::open(&fixture("tone.flac")).unwrap();
    let pos = dec.seek_ms(1000).unwrap();
    assert!((900..=1000).contains(&pos), "seeked to {pos}");
    let mut buf = Vec::new();
    assert!(dec.next_chunk(&mut buf).unwrap());
}

#[test]
fn extension_check() {
    assert!(is_supported_extension(std::path::Path::new("a/B.FLAC")));
    assert!(!is_supported_extension(std::path::Path::new("a/b.opus")));
    assert!(!is_supported_extension(std::path::Path::new("a/noext")));
}
