use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::matching::match_track;
use super::policy::{Thresholds, Verdict};
use super::{open_conflicts, Applied};
use crate::analysis::handler::Analyzer;
use crate::analysis::store;
use crate::domain::{CandidateStatus, Field, FileOrigin};
use crate::fake_analyzer::FakeAnalyzer;
use crate::real_probe::RealProbe;
use crate::{library, meta, pipeline};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../audio/tests/fixtures")
        .join(name)
}

struct Lib {
    conn: Connection,
    dir: tempfile::TempDir,
    analyzer: FakeAnalyzer,
}

impl Lib {
    fn new() -> Self {
        Lib {
            conn: crate::db::open_in_memory().unwrap(),
            dir: tempfile::tempdir().unwrap(),
            analyzer: FakeAnalyzer::default(),
        }
    }

    fn write_song(&self, name: &str, audio: cd_audio::synth::Audio) -> PathBuf {
        let p = self.dir.path().join(name);
        audio.write_wav(&p).unwrap();
        p
    }

    /// Import a file, tag it, analyse it; returns (track, file).
    fn add(&self, path: &Path, tags: (&str, &str, Option<&str>)) -> (String, String) {
        let r = library::register_file(&self.conn, path, None, FileOrigin::Imported, &RealProbe).unwrap();
        self.tag(&r.track_id, tags);
        let a = self.analyzer.analyse(path).unwrap();
        store::store(&self.conn, &r.track_id, &r.file_id, &a).unwrap();
        (r.track_id, r.file_id)
    }

    fn tag(&self, track: &str, (artist, title, mix): (&str, &str, Option<&str>)) {
        meta::set_correction(&self.conn, track, Field::Artist, Some(artist)).unwrap();
        meta::set_correction(&self.conn, track, Field::Title, Some(title)).unwrap();
        meta::set_correction(&self.conn, track, Field::Mix, Some(mix.unwrap_or(""))).unwrap();
    }

    fn matches(&self, track: &str, file: &str) -> Vec<super::matching::MatchOutcome> {
        match_track(&self.conn, &self.analyzer, track, file, &Thresholds::default()).unwrap()
    }

    fn tracks(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM track", [], |r| r.get(0))
            .unwrap()
    }
}

fn song(seed: u64) -> cd_audio::synth::Audio {
    cd_audio::synth::Song::from_seed(seed).render(30.0)
}

#[test]
fn a_reencode_of_an_owned_track_is_merged_with_evidence() {
    let lib = Lib::new();
    let (a, _) = lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    let (b, bf) = lib.add(
        &fixture("identity/song1-96.m4a"),
        ("Fixture Collective", "Song 1", Some("Original Mix")),
    );
    let out = lib.matches(&b, &bf);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].applied, Applied::Merged { kept: a.clone() });
    assert_eq!(lib.tracks(), 1);
    assert_eq!(library::files_for_track(&lib.conn, &a).unwrap().len(), 2);
}

#[test]
fn untagged_reencode_is_found_by_fingerprint_alone() {
    let lib = Lib::new();
    let (a, _) = lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    let r = library::register_file(
        &lib.conn,
        &fixture("identity/song1-128.mp3"),
        None,
        FileOrigin::Imported,
        &RealProbe,
    )
    .unwrap();
    let an = lib.analyzer.analyse(&fixture("identity/song1-128.mp3")).unwrap();
    store::store(&lib.conn, &r.track_id, &r.file_id, &an).unwrap();
    let out = lib.matches(&r.track_id, &r.file_id);
    assert_eq!(out[0].applied, Applied::Merged { kept: a });
}

#[test]
fn a_remix_is_linked_as_a_version() {
    let lib = Lib::new();
    let s = cd_audio::synth::Song::from_seed(5);
    let (a, _) = lib.add(
        &lib.write_song("a.wav", s.render(30.0)),
        ("Fixture Collective", "Song 5", None),
    );
    let (b, bf) = lib.add(
        &lib.write_song("b.wav", s.remix(3).render(30.0)),
        ("Fixture Collective", "Song 5", Some("Test Remix")),
    );
    let out = lib.matches(&b, &bf);
    assert!(
        matches!(out[0].applied, Applied::LinkedAsVersions { .. }),
        "{out:?}"
    );
    assert_eq!(super::versions(&lib.conn, &a).unwrap().len(), 1);
    assert_eq!(lib.tracks(), 2);
}

#[test]
fn a_pitched_copy_is_merged_as_a_variant() {
    let lib = Lib::new();
    let orig = song(2);
    let (a, _) = lib.add(
        &lib.write_song("a.wav", orig.clone()),
        ("Fixture Collective", "Song 2", None),
    );
    let fast = cd_audio::synth::speed(&orig, 1.04);
    let (b, bf) = lib.add(
        &lib.write_song("b.wav", fast),
        ("Fixture Collective", "Song 2", None),
    );
    let out = lib.matches(&b, &bf);
    assert!(
        matches!(out[0].decision.verdict, Verdict::PitchedCopy { .. }),
        "{out:?}"
    );
    let files = library::files_for_track(&lib.conn, &a).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].path.ends_with("a.wav") && files[0].is_primary);
}

#[test]
fn mislabelled_audio_goes_to_review() {
    let lib = Lib::new();
    lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    let (b, bf) = lib.add(
        &lib.write_song("b.wav", song(6)),
        ("Fixture Collective", "Song 1", None),
    );
    let out = lib.matches(&b, &bf);
    assert_eq!(out[0].applied, Applied::SentToReview);
    let c = open_conflicts(&lib.conn).unwrap();
    assert_eq!(c.len(), 1);
    assert!(c[0].reason.contains("audio is different"));
    // Matching again does not reopen or duplicate it.
    lib.matches(&b, &bf);
    assert_eq!(open_conflicts(&lib.conn).unwrap().len(), 1);
}

#[test]
fn unrelated_tracks_are_left_alone() {
    let lib = Lib::new();
    lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    let (b, bf) = lib.add(
        &lib.write_song("b.wav", song(2)),
        ("Fixture Collective", "Song 2", None),
    );
    assert!(lib.matches(&b, &bf).is_empty());
    assert_eq!(lib.tracks(), 2);
}

#[test]
fn a_candidate_the_user_already_owns_is_held_back_not_merged() {
    let lib = Lib::new();
    lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    // A discovery download of the same recording.
    let t = meta::create_track(&lib.conn).unwrap();
    lib.tag(&t, ("Fixture Collective", "Song 1", None));
    lib.conn
        .execute(
            "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES ('c1', ?1, 'analysing', 0, 0)",
            [&t],
        )
        .unwrap();
    let staged = lib.dir.path().join("staged.mp3");
    std::fs::copy(fixture("identity/song1-128.mp3"), &staged).unwrap();
    let r = library::register_staged(&lib.conn, &staged, &t, &RealProbe).unwrap();
    let an = lib.analyzer.analyse(&staged).unwrap();
    store::store(&lib.conn, &t, &r.file_id, &an).unwrap();

    lib.matches(&t, &r.file_id);
    assert_eq!(lib.tracks(), 2, "the owned track is not touched");
    let s = pipeline::state(&lib.conn, "c1").unwrap();
    assert_eq!(s.status, CandidateStatus::Blocked);
    assert!(s.status_reason.unwrap().contains("Already in your library"));
    assert!(!super::matching::candidate_is_active(&lib.conn, "c1").unwrap());
}

#[test]
fn a_candidate_that_is_another_version_of_an_owned_track_says_so() {
    let lib = Lib::new();
    let s = cd_audio::synth::Song::from_seed(5);
    lib.add(
        &lib.write_song("a.wav", s.render(30.0)),
        ("Fixture Collective", "Song 5", None),
    );
    let t = meta::create_track(&lib.conn).unwrap();
    lib.tag(&t, ("Fixture Collective", "Song 5", Some("Test Remix")));
    lib.conn
        .execute(
            "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES ('c1', ?1, 'analysing', 0, 0)",
            [&t],
        )
        .unwrap();
    let staged = lib.write_song("remix.wav", s.remix(3).render(30.0));
    let r = library::register_staged(&lib.conn, &staged, &t, &RealProbe).unwrap();
    let an = lib.analyzer.analyse(&staged).unwrap();
    store::store(&lib.conn, &t, &r.file_id, &an).unwrap();
    lib.matches(&t, &r.file_id);
    let reason: String = lib
        .conn
        .query_row(
            "SELECT reason FROM explanation WHERE candidate_id = 'c1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        reason.contains("Another version of Fixture Collective - Song 5"),
        "{reason}"
    );
    assert!(super::matching::candidate_is_active(&lib.conn, "c1").unwrap());
}

#[test]
fn pairs_the_user_decided_are_not_reconsidered() {
    let lib = Lib::new();
    let (a, _) = lib.add(
        &lib.write_song("a.wav", song(1)),
        ("Fixture Collective", "Song 1", None),
    );
    // A remaster (different bytes, same audio) that the user said is a
    // different track.
    let remaster = cd_audio::synth::remaster(&song(1));
    let (b, bf) = lib.add(
        &lib.write_song("b.wav", remaster),
        ("Fixture Collective", "Song 1", None),
    );
    assert_eq!(lib.tracks(), 2);
    crate::library::duplicates::dismiss(&lib.conn, &a, &b).unwrap();
    assert!(lib.matches(&b, &bf).is_empty());
    assert_eq!(lib.tracks(), 2);
    assert!(open_conflicts(&lib.conn).unwrap().is_empty());
}
