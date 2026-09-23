//! Calibration of the identity policy against labelled cases
//! (tests/identity_cases.json): the same recording across encodings and
//! masters, pitched copies, excerpts, edits, remixes, unrelated songs and
//! mislabelled files. Fails on any wrong verdict, and in particular on any
//! pair wrongly called the same recording.
//!
//! Set CALIBRATION_REPORT=<path> to write the results as Markdown.

use std::collections::HashMap;
use std::path::PathBuf;

use cd_audio::fingerprint::{compare_with_speed, fingerprint_file, fingerprint_samples, Fingerprint};
use cd_audio::synth::{self, Audio, Song};
use cd_core::identity::fingerprint_evidence;
use cd_core::identity::policy::{decide, Evidence, Side, Thresholds, Verdict};
use serde::Deserialize;

const SECONDS: f32 = 30.0;

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    a: Spec,
    b: Spec,
    expect: String,
}

#[derive(Deserialize)]
struct Spec {
    song: u64,
    audio: String,
    title: Option<String>,
    mix: Option<String>,
}

enum Source {
    Samples(Audio),
    File(PathBuf),
    None,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures/identity")
}

struct Songs(HashMap<u64, Audio>);

impl Songs {
    fn original(&mut self, seed: u64) -> Audio {
        self.0
            .entry(seed)
            .or_insert_with(|| Song::from_seed(seed).render(SECONDS))
            .clone()
    }

    fn source(&mut self, spec: &Spec) -> Source {
        let (kind, arg) = spec.audio.split_once(':').unwrap_or((spec.audio.as_str(), ""));
        let range = |s: &str| -> (f32, f32) {
            let (a, b) = s.split_once('-').unwrap();
            (a.parse().unwrap(), b.parse().unwrap())
        };
        match kind {
            "none" => Source::None,
            "file" => Source::File(fixtures().join(arg)),
            "original" => Source::Samples(self.original(spec.song)),
            "remaster" => Source::Samples(synth::remaster(&self.original(spec.song))),
            "section" => {
                let (a, b) = range(arg);
                Source::Samples(synth::section(&self.original(spec.song), a, b))
            }
            "cut" => {
                let (a, b) = range(arg);
                Source::Samples(synth::cut(&self.original(spec.song), a, b))
            }
            "speed" => Source::Samples(synth::speed(&self.original(spec.song), arg.parse().unwrap())),
            "remix" => Source::Samples(
                Song::from_seed(spec.song)
                    .remix(arg.parse().unwrap())
                    .render(SECONDS),
            ),
            other => panic!("unknown audio spec {other}"),
        }
    }
}

fn fingerprint(src: &Source, speed: f64) -> Option<Fingerprint> {
    match src {
        Source::Samples(a) => Some(fingerprint_samples(&a.samples, synth::RATE, 2, speed)),
        Source::File(p) => Some(fingerprint_file(p, speed).unwrap()),
        Source::None => None,
    }
}

fn real_duration_ms(src: &Source) -> Option<u64> {
    match src {
        Source::Samples(a) => Some(a.duration_ms()),
        Source::File(p) => Some(cd_audio::decode::decode_all(p).unwrap().decoded_ms),
        Source::None => None,
    }
}

fn side(spec: &Spec, duration_ms: Option<u64>) -> Side {
    Side {
        artist: Some("Fixture Collective".into()),
        title: Some(
            spec.title
                .clone()
                .unwrap_or_else(|| format!("Song {}", spec.song)),
        ),
        mix: spec.mix.clone(),
        duration_ms: duration_ms.map(|d| d as i64),
        external_ids: vec![],
    }
}

fn verdict_name(v: &Verdict) -> &'static str {
    match v {
        Verdict::SameRecording => "same_recording",
        Verdict::PitchedCopy { .. } => "pitched_copy",
        Verdict::DifferentVersion => "different_version",
        Verdict::Unrelated => "unrelated",
        Verdict::Unknown => "unknown",
        Verdict::NeedsReview { .. } => "needs_review",
    }
}

#[test]
fn policy_matches_every_labelled_case() {
    let cases: Cases =
        serde_json::from_str(include_str!("identity_cases.json")).expect("valid identity_cases.json");
    let mut songs = Songs(HashMap::new());
    let thresholds = Thresholds::default();
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    for case in &cases.cases {
        let (sa, sb) = (songs.source(&case.a), songs.source(&case.b));
        let (da, db) = (real_duration_ms(&sa), real_duration_ms(&sb));
        let mut evidence: Vec<Evidence> = Vec::new();
        let mut measured = None;
        if let (Some(fa), Some(db_ms)) = (fingerprint(&sa, 1.0), db) {
            if let Some((c, speed)) = compare_with_speed(&fa, |s| fingerprint(&sb, s), db_ms) {
                evidence.push(fingerprint_evidence(c.score, c.coverage, speed));
                measured = Some((c.score, c.coverage, speed));
            }
        }
        let d = decide(&side(&case.a, da), &side(&case.b, db), &evidence, &thresholds);
        let got = verdict_name(&d.verdict);
        let wrongly_same = matches!(d.verdict, Verdict::SameRecording | Verdict::PitchedCopy { .. })
            && !matches!(case.expect.as_str(), "same_recording" | "pitched_copy");
        if got != case.expect {
            failures.push(format!(
                "{}: expected {}, got {got} {:?} ({})",
                case.name,
                case.expect,
                measured,
                d.reasons.join(" ")
            ));
        }
        assert!(!wrongly_same, "false merge in case {}", case.name);
        rows.push((case.name.clone(), case.expect.clone(), got, measured));
    }

    let mut report = String::from(
        "| Case | Expected | Result | Score | Coverage | Speed |\n| --- | --- | --- | --- | --- | --- |\n",
    );
    for (name, expect, got, m) in &rows {
        let (s, c, sp) = m
            .map(|(s, c, sp)| (format!("{s:.2}"), format!("{c:.2}"), format!("{sp:.3}")))
            .unwrap_or(("-".into(), "-".into(), "-".into()));
        let ok = if expect == got {
            got.to_string()
        } else {
            format!("**{got}**")
        };
        report.push_str(&format!("| {name} | {expect} | {ok} | {s} | {c} | {sp} |\n"));
    }
    eprintln!("{report}");
    if let Ok(path) = std::env::var("CALIBRATION_REPORT") {
        std::fs::write(path, &report).unwrap();
    }
    assert!(failures.is_empty(), "wrong verdicts:\n{}", failures.join("\n"));
}
