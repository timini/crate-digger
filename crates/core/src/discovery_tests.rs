use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rusqlite::Connection;

use super::*;
use crate::adapters::fake::FakeSource;
use crate::adapters::AdapterError;
use crate::domain::RatingKind;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::{run_one, Handler};

fn proposal(artist: &str, title: &str, kind: &str, url: Option<&str>) -> CandidateProposal {
    CandidateProposal {
        artist: artist.into(),
        title: title.into(),
        mix: None,
        label: Some("Fixture Label".into()),
        release: None,
        reasons: vec![format!("Found by {kind}")],
        evidence: vec![EvidenceProposal {
            source_kind: kind.into(),
            source_url: url.map(str::to_string),
            supplied_text_id: None,
            excerpt: format!("{artist} - {title}"),
            confidence: 0.8,
        }],
    }
}

struct Rig {
    conn: Connection,
    source: Arc<FakeSource>,
    scheduler: Scheduler,
    handlers: HashMap<&'static str, Arc<dyn Handler>>,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new(catalogue: Vec<CandidateProposal>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
        let source = Arc::new(FakeSource::new(catalogue));
        let handler: Arc<dyn Handler> = Arc::new(DiscoverHandler::new(source.clone(), "fake_acquirer"));
        Rig {
            conn,
            source,
            scheduler: Scheduler::new(Limits::default(), Arc::new(staged_bytes)),
            handlers: HashMap::from([(kinds::DISCOVER, handler)]),
            _dir: dir,
        }
    }

    fn run(&mut self) {
        self.run_kind(kinds::DISCOVER);
    }

    fn run_kind(&mut self, kind: &str) {
        let stop = AtomicBool::new(false);
        run_one(
            &mut self.conn,
            &self.scheduler,
            &self.handlers,
            &[kind],
            "w",
            &stop,
        )
        .unwrap();
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

#[test]
fn model_only_suggestions_stay_unverified_and_are_never_acquired() {
    let mut rig = Rig::new(vec![
        proposal(
            "Model Artist",
            "Imagined",
            LLM_EVIDENCE,
            Some("https://example.invalid/claimed"),
        ),
        proposal(
            "Page Artist",
            "Listed",
            "page",
            Some("https://example.invalid/list"),
        ),
    ]);
    request_discovery(&rig.conn, "fake_source", 10, 1).unwrap();
    rig.run();

    let verified: Vec<(String, bool)> = rig
        .conn
        .prepare("SELECT m.artist, c.verified FROM candidate c JOIN track_meta m USING (track_id) ORDER BY m.artist")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        verified,
        vec![("Model Artist".into(), false), ("Page Artist".into(), true)]
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM job WHERE kind = 'acquire'"), 1);
    let refused = queue_acquisition(
        &rig.conn,
        &rig.conn
            .query_row("SELECT id FROM candidate WHERE verified = 0", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "fake_acquirer",
        2,
    );
    assert!(refused.is_err());

    let run = &recent_runs(&rig.conn, 1).unwrap()[0];
    assert_eq!(
        (run.outcome.as_str(), run.created, run.unverified),
        ("found", 2, 1)
    );
}

#[test]
fn later_evidence_verifies_an_existing_suggestion_and_queues_it() {
    let mut rig = Rig::new(vec![proposal(
        "Model Artist",
        "Imagined",
        "discogs",
        Some("https://www.discogs.com/release/1"),
    )]);
    ingest(
        &rig.conn,
        "llm",
        &[proposal(
            "Model Artist",
            "Imagined",
            LLM_EVIDENCE,
            Some("https://example.invalid/x"),
        )],
        1,
    )
    .unwrap();
    let id: String = rig
        .conn
        .query_row("SELECT id FROM candidate", [], |r| r.get(0))
        .unwrap();
    identify(&rig.conn, &id, 1).unwrap();

    request_discovery(&rig.conn, "fake_source", 10, 2).unwrap();
    rig.run();
    assert_eq!(rig.count("SELECT COUNT(*) FROM candidate"), 1);
    assert_eq!(rig.count("SELECT verified FROM candidate"), 1);
    assert_eq!(rig.count("SELECT COUNT(*) FROM evidence"), 2);
    assert_eq!(rig.count("SELECT COUNT(*) FROM job WHERE kind = 'acquire'"), 1);

    // The same evidence again adds nothing.
    ingest(&rig.conn, "fake_source", &rig.source.catalogue.clone(), 3).unwrap();
    assert_eq!(rig.count("SELECT COUNT(*) FROM evidence"), 2);
    assert_eq!(rig.count("SELECT COUNT(*) FROM explanation"), 2);
}

#[test]
fn failed_retrieval_is_recorded_apart_from_an_empty_run() {
    let mut rig = Rig::new(vec![]);
    rig.source.failure.set(Some(AdapterError::Unavailable(
        "Cannot reach the service.".into(),
    )));
    request_discovery(&rig.conn, "fake_source", 10, 1).unwrap();
    rig.run();
    let failed = &recent_runs(&rig.conn, 1).unwrap()[0];
    assert_eq!(failed.outcome, "failed");
    assert!(failed.detail.as_deref().unwrap().contains("Cannot reach"));

    rig.source.failure.set(None);
    rig.conn.execute("UPDATE job SET next_run_at = 0", []).unwrap();
    rig.run();
    let runs = recent_runs(&rig.conn, 5).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].outcome, "empty");
    assert_eq!(runs[0].detail, None);
}

#[test]
fn pasted_text_and_pages_reach_the_source() {
    let mut rig = Rig::new(vec![]);
    let text_id = save_supplied_text(&rig.conn, Some("Forum post"), "Artist - Title", 1).unwrap();
    assert!(save_supplied_text(&rig.conn, None, "   ", 1).is_err());
    request_input(
        &rig.conn,
        "fake_source",
        DiscoveryJob::Text {
            supplied_text_id: text_id.clone(),
        },
        5,
        1,
    )
    .unwrap();
    rig.run();
    request_input(
        &rig.conn,
        "fake_source",
        DiscoveryJob::Page {
            url: "https://example.invalid/tracklist".into(),
        },
        5,
        2,
    )
    .unwrap();
    rig.run();
    let requests = rig.source.requests.lock().unwrap();
    assert_eq!(
        requests[0].input,
        DiscoveryInput::Text {
            supplied_text_id: text_id,
            text: "Artist - Title".into()
        }
    );
    assert_eq!(
        requests[1].input,
        DiscoveryInput::Page {
            url: "https://example.invalid/tracklist".into()
        }
    );
    let inputs: Vec<String> = recent_runs(&rig.conn, 5)
        .unwrap()
        .into_iter()
        .map(|r| r.input)
        .collect();
    assert_eq!(
        inputs,
        vec!["page https://example.invalid/tracklist", "pasted text"]
    );
}

#[test]
fn positive_ratings_expand_seeds() {
    let rig = Rig::new(vec![]);
    rig.conn
        .execute(
            "INSERT INTO seed (id, kind, value, created_at) VALUES ('s', 'artist', 'Liked Artist', 1)",
            [],
        )
        .unwrap();
    let rate = |artist: &str, label: &str, kind: RatingKind| {
        let track = meta::create_track(&rig.conn).unwrap();
        meta::set_extracted(
            &rig.conn,
            &track,
            "test",
            &[
                (Field::Artist, Some(artist.into())),
                (Field::Label, Some(label.into())),
            ],
        )
        .unwrap();
        crate::review::rate(&rig.conn, &track, kind, "session", 1).unwrap();
    };
    rate("liked artist", "Good Label", RatingKind::Star2);
    rate("Disliked Artist", "Bad Label", RatingKind::ThumbsDown);
    let seeds = seeds_for_run(&rig.conn).unwrap();
    let values: Vec<(SeedKind, &str)> = seeds.iter().map(|s| (s.kind, s.value.as_str())).collect();
    assert_eq!(
        values,
        vec![
            (SeedKind::Artist, "Liked Artist"),
            (SeedKind::Label, "Good Label")
        ]
    );
}

#[test]
fn refresh_is_due_after_the_interval_unless_a_run_is_waiting() {
    let mut rig = Rig::new(vec![]);
    const SIX_HOURS: i64 = 6 * 3_600_000;
    assert!(refresh_due(&rig.conn, "fake_source", SIX_HOURS, 1_000).unwrap());
    request_discovery(&rig.conn, "fake_source", 5, 1_000).unwrap();
    assert!(!refresh_due(&rig.conn, "fake_source", SIX_HOURS, 1_000).unwrap());
    rig.run();
    let started = recent_runs(&rig.conn, 1).unwrap()[0].started_at;
    assert!(!refresh_due(&rig.conn, "fake_source", SIX_HOURS, started + SIX_HOURS - 1).unwrap());
    assert!(refresh_due(&rig.conn, "fake_source", SIX_HOURS, started + SIX_HOURS).unwrap());
    // Page and pasted-text runs do not count as a refresh.
    assert!(refresh_due(&rig.conn, "other_source", SIX_HOURS, started).unwrap());
}

#[test]
fn verified_candidates_wait_visibly_when_no_download_source_is_set_up() {
    let mut rig = Rig::new(vec![proposal(
        "Page Artist",
        "Listed",
        "page",
        Some("https://example.invalid/list"),
    )]);
    let staging = rig._dir.path().join("staging");
    rig.handlers.insert(
        kinds::ACQUIRE,
        Arc::new(crate::acquisition::AcquireHandler {
            acquirers: vec![],
            staging_root: staging,
            probe: Arc::new(crate::real_probe::RealProbe),
            poll: std::time::Duration::from_millis(10),
            queued_watch: crate::acquisition::QUEUED_WATCH,
        }),
    );
    request_discovery(&rig.conn, "fake_source", 10, 1).unwrap();
    rig.run();
    rig.run_kind(kinds::ACQUIRE);
    let (state, reason): (String, String) = rig
        .conn
        .query_row("SELECT state, reason FROM job WHERE kind = 'acquire'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(state, "paused");
    assert!(reason.contains("Connect Soulseek in Settings"), "{reason}");
    assert_eq!(
        pipeline::state(
            &rig.conn,
            &rig.conn
                .query_row("SELECT id FROM candidate", [], |r| r.get::<_, String>(0))
                .unwrap()
        )
        .unwrap()
        .stage,
        Stage::AcquisitionQueued
    );
}

#[test]
fn new_candidates_get_a_youtube_lookup_only_when_the_route_asks() {
    let catalogue = vec![proposal(
        "Page Artist",
        "Listed",
        "page",
        Some("https://example.invalid/list"),
    )];
    let mut rig = Rig::new(catalogue.clone());
    request_discovery(&rig.conn, "fake_source", 10, 1).unwrap();
    rig.run();
    assert_eq!(rig.count("SELECT COUNT(*) FROM job WHERE kind = 'youtube'"), 0);

    let mut rig = Rig::new(catalogue);
    let handler: Arc<dyn Handler> =
        Arc::new(DiscoverHandler::new(rig.source.clone(), "fake_acquirer").with_video_lookup());
    rig.handlers.insert(kinds::DISCOVER, handler);
    request_discovery(&rig.conn, "fake_source", 10, 1).unwrap();
    rig.run();
    assert_eq!(rig.count("SELECT COUNT(*) FROM job WHERE kind = 'youtube'"), 1);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM youtube_lookup WHERE status = 'queued'"),
        1
    );
}
