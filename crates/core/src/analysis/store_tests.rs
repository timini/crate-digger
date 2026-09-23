use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::Connection;

use super::handler::{AnalysisHandler, Analyzer};
use super::store::{self, plan};
use super::FeatureVersion;
use crate::domain::{Field, FileOrigin, JobState};
use crate::fake_analyzer::FakeAnalyzer;
use crate::jobs::kinds;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::{run_one, Handler};
use crate::real_probe::RealProbe;
use crate::{library, meta};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures")
}

fn import(conn: &Connection, dir: &Path, name: &str) -> (String, String) {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(format!("copy-{name}"));
    std::fs::copy(fixtures().join(name), &p).unwrap();
    let r = library::register_file(conn, &p, None, FileOrigin::Imported, &RealProbe).unwrap();
    (r.track_id, r.file_id)
}

fn v(model: &str) -> FeatureVersion {
    FeatureVersion {
        model_id: model.into(),
        weights_checksum: "none".into(),
        preprocessing_version: "p1".into(),
    }
}

fn run_analysis(conn: &mut Connection, analyzer: FakeAnalyzer) -> usize {
    let s = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    let h: Arc<dyn Handler> = Arc::new(AnalysisHandler {
        analyzer: Arc::new(analyzer),
        after: None,
    });
    let handlers = HashMap::from([(h.kind(), h)]);
    let mut n = 0;
    while run_one(
        conn,
        &s,
        &handlers,
        &[kinds::ANALYSE],
        "w",
        &AtomicBool::new(false),
    )
    .unwrap()
    {
        n += 1;
    }
    n
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn stores_features_fingerprint_and_fills_missing_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_in_memory().unwrap();
    let (track, file) = import(&conn, dir.path(), "tone.wav");
    let p = plan(&conn, &v("fake-v1"), 1).unwrap();
    assert_eq!(p.queued, 1);
    assert_eq!(run_analysis(&mut conn, FakeAnalyzer::default()), 1);

    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM feature_record"),
        2,
        "one segment and the summary"
    );
    assert!(store::summary_embedding(&conn, &track, &v("fake-v1"))
        .unwrap()
        .is_some());
    assert!(store::fingerprint_of_file(&conn, &file).unwrap().is_some());
    assert_eq!(
        store::status(&conn, &track, &v("fake-v1"))
            .unwrap()
            .unwrap()
            .state,
        "done"
    );
    // The WAV has no tempo tag, so the analysed tempo fills it in.
    assert_eq!(meta::effective(&conn, &track).unwrap().tempo, Some(124.0));
    let waveform: Option<Vec<u8>> = conn
        .query_row("SELECT waveform FROM audio_file WHERE id = ?1", [&file], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(waveform.map(|w| w.len()), Some(800));
    // Planning again queues nothing.
    assert_eq!(plan(&conn, &v("fake-v1"), 2).unwrap(), store::Plan::default());
}

#[test]
fn tag_values_and_user_edits_beat_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_in_memory().unwrap();
    let (track, _) = import(&conn, dir.path(), "tone.flac"); // tagged 124 BPM, 8A
    meta::set_correction(&conn, &track, Field::MusicalKey, Some("9A")).unwrap();
    plan(&conn, &v("fake-v1"), 1).unwrap();
    let a = FakeAnalyzer::default();
    run_analysis(&mut conn, a);
    let m = meta::effective(&conn, &track).unwrap();
    assert_eq!(m.musical_key.as_deref(), Some("9A"));
    assert_eq!(m.tempo, Some(124.0));
}

#[test]
fn clearing_temporary_audio_keeps_features_and_fingerprints() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_in_memory().unwrap();
    let t = meta::create_track(&conn).unwrap();
    let staged = dir.path().join("staging/c1");
    std::fs::create_dir_all(&staged).unwrap();
    let path = staged.join("a.wav");
    std::fs::copy(fixtures().join("tone.wav"), &path).unwrap();
    library::register_staged(&conn, &path, &t, &RealProbe).unwrap();
    conn.execute(
        "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES ('c1', ?1, 'reviewed', 0, 0)",
        [&t],
    )
    .unwrap();
    plan(&conn, &v("fake-v1"), 1).unwrap();
    run_analysis(&mut conn, FakeAnalyzer::default());

    let cleared = crate::archive::clear_temporary(&conn, false).unwrap();
    assert_eq!(cleared.removed_files, 1);
    assert!(!path.exists());
    assert!(store::summary_embedding(&conn, &t, &v("fake-v1"))
        .unwrap()
        .is_some());
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM fingerprint WHERE audio_file_id IS NULL"
        ),
        1
    );
}

#[test]
fn upgrade_queues_where_audio_exists_and_marks_the_rest_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_in_memory().unwrap();
    let (with_audio, _) = import(&conn, dir.path(), "tone.wav");
    let (gone, gone_file) = import(&conn, dir.path(), "tone.flac");
    plan(&conn, &v("fake-v1"), 1).unwrap();
    run_analysis(&mut conn, FakeAnalyzer::default());

    // The second track's audio disappears, then the model is upgraded.
    let f = library::file(&conn, &gone_file).unwrap();
    std::fs::remove_file(&f.path).unwrap();
    library::mark_missing(&conn, &f.id, &f.path).unwrap();
    let p = plan(&conn, &v("fake-v2"), 2).unwrap();
    assert_eq!((p.queued, p.needs_audio), (1, 1));
    let s = store::status(&conn, &gone, &v("fake-v2")).unwrap().unwrap();
    assert_eq!(s.state, "needs_audio");
    assert!(s.reason.unwrap().contains("Relink"));

    // Planning again adds no jobs and changes nothing: no retry loop.
    let jobs_before = count(&conn, "SELECT COUNT(*) FROM job");
    let again = plan(&conn, &v("fake-v2"), 3).unwrap();
    assert_eq!((again.queued, again.needs_audio), (0, 1));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM job"), jobs_before);

    // The v2 analysis runs for the track with audio; the old features stay.
    let v2 = FakeAnalyzer {
        version: v("fake-v2"),
    };
    assert_eq!(run_analysis(&mut conn, v2), 1);
    assert!(store::summary_embedding(&conn, &with_audio, &v("fake-v2"))
        .unwrap()
        .is_some());
    assert!(store::summary_embedding(&conn, &gone, &v("fake-v1"))
        .unwrap()
        .is_some());
    assert!(store::summary_embedding(&conn, &gone, &v("fake-v2"))
        .unwrap()
        .is_none());

    // Old and new versions are never compared.
    let a = store::summary_embedding(&conn, &with_audio, &v("fake-v2"))
        .unwrap()
        .unwrap();
    let b = store::summary_embedding(&conn, &gone, &v("fake-v1"))
        .unwrap()
        .unwrap();
    assert!(a.similarity(&b).is_err());

    // When the audio comes back, the upgrade is queued.
    std::fs::copy(fixtures().join("tone.flac"), &f.path).unwrap();
    library::check_files(&conn, &RealProbe).unwrap();
    assert_eq!(plan(&conn, &v("fake-v2"), 4).unwrap().queued, 1);
}

#[test]
fn a_file_that_cannot_be_decoded_fails_the_job_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = crate::db::open_in_memory().unwrap();
    let (track, file) = import(&conn, dir.path(), "tone.wav");
    plan(&conn, &v("fake-v1"), 1).unwrap();
    // Replace the file with garbage after import.
    let f = library::file(&conn, &file).unwrap();
    std::fs::copy(fixtures().join("corrupt.mp3"), &f.path).unwrap();
    run_analysis(&mut conn, FakeAnalyzer::default());

    let failed = crate::jobs::list(&conn, &[JobState::Failed], 5).unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].reason.as_ref().unwrap().starts_with("Analysis failed"));
    let s = store::status(&conn, &track, &v("fake-v1")).unwrap().unwrap();
    assert_eq!(s.state, "failed");
    let f = library::file(&conn, &file).unwrap();
    assert_eq!(f.availability, crate::domain::Availability::Corrupt);
    // Planning does not requeue a failed analysis by itself.
    assert_eq!(plan(&conn, &v("fake-v1"), 2).unwrap().queued, 0);
}

#[test]
fn analyzer_trait_reports_its_version() {
    assert_eq!(FakeAnalyzer::default().version(), v("fake-v1"));
}
