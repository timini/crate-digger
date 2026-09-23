//! Feature extraction on synthetic material with known answers.

use cd_analyzer::frames::FrameAnalyzer;
use cd_audio::synth::Song;

const RATE: u32 = 44_100;
const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// I-IV-V-I (or i-iv-V-i) cadences, one second per chord, as sine triads.
fn cadence(root: usize, minor: bool) -> Vec<f32> {
    let third = if minor { 3 } else { 4 };
    let chords: [[i32; 3]; 4] = [[0, third, 7], [5, 5 + third, 12], [7, 11, 14], [0, third, 7]];
    let mut out = Vec::new();
    for _ in 0..4 {
        for chord in chords {
            for i in 0..RATE {
                let t = i as f32 / RATE as f32;
                let v: f32 = chord
                    .iter()
                    .map(|n| {
                        let f = 261.63 * 2f32.powf((root as i32 + n) as f32 / 12.0);
                        (std::f32::consts::TAU * f * t).sin()
                    })
                    .sum::<f32>()
                    * 0.2;
                out.push(v);
            }
        }
    }
    out
}

#[test]
fn key_of_cadences_in_all_24_keys() {
    let mut wrong = Vec::new();
    for (root, name) in NAMES.iter().enumerate() {
        for minor in [false, true] {
            let mut fa = FrameAnalyzer::new(RATE);
            fa.push(&cadence(root, minor));
            let k = cd_analyzer::key::estimate(&fa.chroma_frames).expect("a key");
            let expected = format!("{name} {}", if minor { "minor" } else { "major" });
            if k.name != expected {
                wrong.push(format!("{expected} read as {}", k.name));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:?}");
}

#[test]
fn tempo_of_synthetic_songs_within_half_a_bpm() {
    for seed in 1..=12u64 {
        let song = Song::from_seed(seed);
        let audio = song.render(60.0);
        let mono: Vec<f32> = audio.samples.chunks(2).map(|c| (c[0] + c[1]) / 2.0).collect();
        let mut fa = FrameAnalyzer::new(RATE);
        fa.push(&mono);
        let (bpm, _) = cd_analyzer::tempo::estimate(&fa.frames, fa.frames_per_second()).expect("a tempo");
        assert!(
            (bpm - song.bpm).abs() <= 0.5,
            "seed {seed}: {bpm} vs {}",
            song.bpm
        );
    }
}

#[test]
fn full_analysis_of_a_long_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.wav");
    Song::from_seed(7).render(100.0).write_wav(&path).unwrap();
    let a = cd_analyzer::analyse(&path, &|_| {}).unwrap();
    assert_eq!(a.duration_ms, 100_000);
    assert_eq!(a.segments.len(), 3, "three 30 s windows for a long track");
    assert_eq!(a.segments[1].start_ms, 35_000);
    assert_eq!(a.embeddings.len(), 4, "three segments and the mean");
    assert!(a
        .embeddings
        .iter()
        .all(|e| e.vector.len() == 96 && e.version.model_id == "cd-dsp-v1"));
    assert!(a.loudness_lufs.unwrap() < -5.0 && a.loudness_lufs.unwrap() > -40.0);
    assert_eq!(a.quality.clipping_ratio, 0.0);
    assert!(a.quality.silence_ratio < 0.05);
    assert!(!a.fingerprint.data.is_empty());
    assert!((a.tempo_bpm.unwrap() - 121.0).abs() <= 0.5);
}

#[test]
fn short_files_use_one_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("short.wav");
    Song::from_seed(2).render(20.0).write_wav(&path).unwrap();
    let a = cd_analyzer::analyse(&path, &|_| {}).unwrap();
    assert_eq!(a.segments.len(), 1);
    assert_eq!(a.embeddings.len(), 2);
}
