use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection};

use super::*;
use crate::acquisition::{AcquireHandler, AnalyseHandler, ValidateHandler};
use crate::adapters::demo::DemoAcquirer;
use crate::adapters::fake::{FakeAcquirer, FakeSource};
use crate::adapters::{CandidateProposal, EvidenceProposal};
use crate::discovery::{self, DiscoverHandler};
use crate::domain::{CandidateStatus, JobState};
use crate::jobs::kinds;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::{run_one, Handler};

use crate::real_probe::RealProbe;

const S1: &str = "session-1";
const S2: &str = "session-2";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures")
}

/// A ready candidate backed by a copy of `fixture` in `dir`.
fn ready_candidate(conn: &Connection, dir: &Path, name: &str, fixture: &str) -> (String, String) {
    let proposal = CandidateProposal {
        artist: "Artist".into(),
        title: name.into(),
        mix: None,
        label: None,
        release: None,
        reasons: vec!["Because".into()],
        evidence: vec![EvidenceProposal {
            source_kind: "test".into(),
            source_url: Some(format!("https://example.invalid/{name}")),
            supplied_text_id: None,
            excerpt: format!("Artist - {name}"),
            confidence: 0.8,
        }],
    };
    let c = discovery::ingest(conn, "test", &[proposal], 1)
        .unwrap()
        .created
        .remove(0);
    let track: String = conn
        .query_row("SELECT track_id FROM candidate WHERE id = ?1", [&c], |r| r.get(0))
        .unwrap();
    let path = dir.join(format!("{name}.flac"));
    std::fs::copy(fixtures().join(fixture), &path).unwrap();
    library::register_staged(conn, &path, &track, &RealProbe).unwrap();
    for s in [Stage::Identified, Stage::Validating, Stage::Analysing] {
        pipeline::transition(conn, &c, s, 1).unwrap();
    }
    mark_ready(conn, &c, 1).unwrap();
    (c, track)
}

fn queue_tracks(conn: &Connection, session: &str) -> Vec<String> {
    next(conn, session, 100)
        .unwrap()
        .into_iter()
        .map(|c| c.track_id)
        .collect()
}

#[test]
fn rating_is_persisted_and_moves_the_card_out_of_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let track = {
        let conn = crate::db::open(&db).unwrap();
        let (_, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
        rate(&conn, &t, RatingKind::Star2, S1, 10).unwrap();
        t
    };
    // Reopen: the rating survived.
    let conn = crate::db::open(&db).unwrap();
    assert_eq!(effective_rating(&conn, &track).unwrap(), Some(RatingKind::Star2));
    assert!(queue_tracks(&conn, S2).is_empty());
    assert_eq!(stats(&conn, S1).unwrap().reviewed, 1);
}

#[test]
fn every_rating_kind_is_distinct_and_skip_or_undo_are_not_ratings() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (_, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    for kind in [
        RatingKind::ThumbsDown,
        RatingKind::Star1,
        RatingKind::Star2,
        RatingKind::Star3,
    ] {
        rate(&conn, &t, kind, S1, 1).unwrap();
        assert_eq!(effective_rating(&conn, &t).unwrap(), Some(kind));
    }
    assert!(rate(&conn, &t, RatingKind::Skip, S1, 1).is_err());
    assert!(rate(&conn, &t, RatingKind::Undo, S1, 1).is_err());
}

#[test]
fn skip_defers_for_the_session_only_and_is_not_a_rating() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (_, a) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    let (_, b) = ready_candidate(&conn, dir.path(), "b", "tone.mp3");
    skip(&conn, &a, S1, 1).unwrap();
    assert_eq!(queue_tracks(&conn, S1), vec![b.clone()]);
    assert_eq!(effective_rating(&conn, &a).unwrap(), None);
    assert_eq!(stats(&conn, S1).unwrap().skipped, 1);
    // A new session (app restart) shows it again.
    assert_eq!(queue_tracks(&conn, S2).len(), 2);
}

#[test]
fn undo_reverses_the_last_rating_then_the_last_skip() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (_, a) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    let (_, b) = ready_candidate(&conn, dir.path(), "b", "tone.mp3");
    skip(&conn, &a, S1, 1).unwrap();
    rate(&conn, &b, RatingKind::ThumbsDown, S1, 2).unwrap();
    assert!(queue_tracks(&conn, S1).is_empty());

    let u = undo(&conn, S1, 3).unwrap().unwrap();
    assert_eq!(
        (u.track_id.as_str(), u.kind, u.effective),
        (b.as_str(), RatingKind::ThumbsDown, None)
    );
    assert_eq!(
        queue_tracks(&conn, S1),
        vec![b.clone()],
        "undone rating returns to the queue"
    );

    let u = undo(&conn, S1, 4).unwrap().unwrap();
    assert_eq!((u.track_id.as_str(), u.kind), (a.as_str(), RatingKind::Skip));
    assert_eq!(queue_tracks(&conn, S1).len(), 2);
    assert!(undo(&conn, S1, 5).unwrap().is_none(), "nothing left to undo");
}

#[test]
fn undo_restores_the_previous_rating() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (c, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    rate(&conn, &t, RatingKind::Star1, S1, 1).unwrap();
    rate(&conn, &t, RatingKind::Star3, S1, 2).unwrap();
    let u = undo(&conn, S1, 3).unwrap().unwrap();
    assert_eq!(u.effective, Some(RatingKind::Star1));
    assert_eq!(effective_rating(&conn, &t).unwrap(), Some(RatingKind::Star1));
    assert_eq!(pipeline::state(&conn, &c).unwrap().stage, Stage::Reviewed);
}

#[test]
fn undo_only_affects_its_own_session() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (_, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    rate(&conn, &t, RatingKind::Star2, S1, 1).unwrap();
    assert!(undo(&conn, S2, 2).unwrap().is_none());
    assert_eq!(effective_rating(&conn, &t).unwrap(), Some(RatingKind::Star2));
}

#[test]
fn rating_does_not_keep_audio_and_keep_does_not_rate() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let (_, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    rate(&conn, &t, RatingKind::Star3, S1, 1).unwrap();
    assert!(!is_kept(&conn, &t).unwrap());
    assert!(!playlists::is_retained(&conn, &t).unwrap());
    let (_, u) = ready_candidate(&conn, dir.path(), "b", "tone.mp3");
    keep(&conn, &u, 1).unwrap();
    assert_eq!(effective_rating(&conn, &u).unwrap(), None);
    assert!(is_kept(&conn, &u).unwrap());
    unkeep(&conn, &u).unwrap();
    assert!(!is_kept(&conn, &u).unwrap());
}

#[test]
fn tracks_without_local_audio_are_never_ready() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    // A candidate with only a YouTube link.
    let p = CandidateProposal {
        artist: "Artist".into(),
        title: "Only on YouTube".into(),
        mix: None,
        label: None,
        release: None,
        reasons: vec![],
        evidence: vec![EvidenceProposal {
            source_kind: "test".into(),
            source_url: Some("https://example.invalid/x".into()),
            supplied_text_id: None,
            excerpt: "x".into(),
            confidence: 0.9,
        }],
    };
    let c = discovery::ingest(&conn, "test", &[p], 1)
        .unwrap()
        .created
        .remove(0);
    let track: String = conn
        .query_row("SELECT track_id FROM candidate WHERE id = ?1", [&c], |r| r.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO youtube_match (id, track_id, video_id, url, looked_up_at, confidence, preferred)
         VALUES ('y', ?1, 'abc', 'https://www.youtube.com/watch?v=abc', 1, 0.9, 1)",
        params![track],
    )
    .unwrap();
    for s in [Stage::Identified, Stage::Validating, Stage::Analysing] {
        pipeline::transition(&conn, &c, s, 1).unwrap();
    }
    assert!(mark_ready(&conn, &c, 1).is_err());

    // Even if forced into Ready, the queue ignores it.
    conn.execute("UPDATE candidate SET stage = 'ready' WHERE id = ?1", [&c])
        .unwrap();
    assert!(queue_tracks(&conn, S1).is_empty());
    assert_eq!(stats(&conn, S1).unwrap().ready, 0);

    // A ready track whose file goes missing drops out of the queue too.
    let (_, t) = ready_candidate(&conn, dir.path(), "a", "tone.flac");
    let f = library::playable_file(&conn, &t).unwrap().unwrap();
    library::mark_missing(&conn, &f.id, &f.path).unwrap();
    assert!(queue_tracks(&conn, S1).is_empty());
}

#[test]
fn cards_show_evidence_reasons_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    ready_candidate(&conn, dir.path(), "a", "tone.flac");
    let card = next(&conn, S1, 1).unwrap().remove(0);
    assert_eq!(card.meta.title.as_deref(), Some("a"));
    assert_eq!(card.reasons, vec!["Because".to_string()]);
    assert_eq!(
        card.evidence[0].source_url.as_deref(),
        Some("https://example.invalid/a")
    );
    assert!(card.verified);
    assert_eq!(card.file.origin, crate::domain::FileOrigin::Staged);
}

#[test]
fn unverified_candidates_cannot_be_queued_for_acquisition() {
    let conn = crate::db::open_in_memory().unwrap();
    let p = CandidateProposal {
        artist: "LLM".into(),
        title: "Hallucination".into(),
        mix: None,
        label: None,
        release: None,
        reasons: vec![],
        evidence: vec![],
    };
    let c = discovery::ingest(&conn, "llm", &[p], 1)
        .unwrap()
        .created
        .remove(0);
    discovery::identify(&conn, &c, 1).unwrap();
    assert!(discovery::queue_acquisition(&conn, &c, "demo", 1).is_err());
}

// ---------------------------------------------------------------------------
// The whole pipeline through the job system.
// ---------------------------------------------------------------------------

struct Rig {
    conn: Connection,
    scheduler: Scheduler,
    handlers: HashMap<&'static str, Arc<dyn Handler>>,
}

impl Rig {
    fn new(conn: Connection, acquirer: Arc<dyn crate::adapters::Acquirer>, staging: PathBuf) -> Self {
        let probe: Arc<dyn library::AudioProbe> = Arc::new(RealProbe);
        let handlers: Vec<Arc<dyn Handler>> = vec![
            Arc::new(DiscoverHandler {
                source: Arc::new(FakeSource::demo()),
                acquirer_id: acquirer.id().to_string(),
            }),
            Arc::new(AcquireHandler {
                acquirer,
                staging_root: staging,
                probe: probe.clone(),
                poll: Duration::from_millis(10),
            }),
            Arc::new(ValidateHandler { probe: probe.clone() }),
            Arc::new(AnalyseHandler { probe }),
        ];
        Rig {
            conn,
            scheduler: Scheduler::new(Limits::default(), Arc::new(staged_bytes)),
            handlers: handlers.into_iter().map(|h| (h.kind(), h)).collect(),
        }
    }

    fn drain(&mut self) {
        let kinds = [kinds::DISCOVER, kinds::ACQUIRE, kinds::VALIDATE, kinds::ANALYSE];
        let stop = AtomicBool::new(false);
        while run_one(
            &mut self.conn,
            &self.scheduler,
            &self.handlers,
            &kinds,
            "w",
            &stop,
        )
        .unwrap()
        {}
    }
}

#[test]
fn discovery_to_ready_through_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
    let mut rig = Rig::new(
        conn,
        Arc::new(DemoAcquirer { seconds: 2 }),
        dir.path().join("staging"),
    );
    discovery::request_discovery(&rig.conn, "fake_source", 3, 1).unwrap();
    rig.drain();

    let s = stats(&rig.conn, S1).unwrap();
    assert_eq!(s.ready, 3, "{s:?}");
    let cards = next(&rig.conn, S1, 10).unwrap();
    assert_eq!(cards.len(), 3);
    for c in &cards {
        assert!(c.file.path.contains("staging"));
        assert_eq!(c.file.duration_ms, Some(2000));
        let waveform: Option<Vec<u8>> = rig
            .conn
            .query_row(
                "SELECT waveform FROM audio_file WHERE id = ?1",
                [&c.file.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(waveform.map(|w| w.len()), Some(800));
    }
    // Nothing failed and no job is left over.
    let open: i64 = rig
        .conn
        .query_row("SELECT COUNT(*) FROM job WHERE state != 'done'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(open, 0);

    // Running discovery again adds new tracks, not repeats.
    discovery::request_discovery(&rig.conn, "fake_source", 3, 2).unwrap();
    rig.drain();
    assert_eq!(stats(&rig.conn, S1).unwrap().ready, 6);
}

#[test]
fn bad_download_fails_the_candidate_without_a_dislike() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
    let bad = Arc::new(FakeAcquirer::new(vec![fixtures().join("corrupt.mp3")]));
    let mut rig = Rig::new(conn, bad, dir.path().join("staging"));
    discovery::request_discovery(&rig.conn, "fake_source", 1, 1).unwrap();
    rig.drain();

    let (status, reason): (CandidateStatus, String) = rig
        .conn
        .query_row("SELECT status, status_reason FROM candidate", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(status, CandidateStatus::Failed);
    assert!(reason.contains("does not affect your ratings"), "{reason}");
    let ratings: i64 = rig
        .conn
        .query_row("SELECT COUNT(*) FROM rating_event", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ratings, 0, "a failed download must not create a rating");
    assert_eq!(stats(&rig.conn, S1).unwrap().ready, 0);
    let failed_jobs = crate::jobs::list(&rig.conn, &[JobState::Failed], 10).unwrap();
    assert_eq!(failed_jobs.len(), 1);
    assert!(failed_jobs[0].reason.is_some());
}

#[test]
fn restart_mid_pipeline_resumes_without_repeating_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let staging = dir.path().join("staging");
    let src = fixtures().join("tone.flac");
    let acquirer = Arc::new(FakeAcquirer::new(vec![src]));
    {
        let mut rig = Rig::new(crate::db::open(&db).unwrap(), acquirer.clone(), staging.clone());
        discovery::request_discovery(&rig.conn, "fake_source", 2, 1).unwrap();
        let stop = AtomicBool::new(false);
        // Run discovery and one acquisition, then "crash".
        for _ in 0..2 {
            run_one(
                &mut rig.conn,
                &rig.scheduler,
                &rig.handlers,
                &[kinds::DISCOVER, kinds::ACQUIRE],
                "w",
                &stop,
            )
            .unwrap();
        }
        rig.conn
            .execute(
                "UPDATE job SET state = 'running', lease_owner = 'dead' WHERE kind = 'validate'",
                [],
            )
            .unwrap();
    }
    let conn = crate::db::open(&db).unwrap();
    crate::jobs::recover_on_start(&conn, crate::util::now_ms()).unwrap();
    let mut rig = Rig::new(conn, acquirer.clone(), staging);
    rig.drain();
    assert_eq!(stats(&rig.conn, S1).unwrap().ready, 2);
    assert_eq!(
        acquirer.started.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "each candidate downloaded once"
    );
}
