use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::*;
use crate::adapters::{AdapterResult, CandidateProposal, EvidenceProposal};
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::run_one;

/// A Soulseek-like source: fixed search results, scripted transfer states.
struct Scripted {
    results: Vec<SearchResult>,
    states: Mutex<VecDeque<TransferStatus>>,
    enqueued: Mutex<Vec<String>>,
    file: PathBuf,
}

impl Acquirer for Scripted {
    fn id(&self) -> &str {
        "slskd"
    }
    fn search(&self, _: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>> {
        Ok(self.results.clone())
    }
    fn enqueue(&self, result: &SearchResult, _: &str, _: &Path) -> AdapterResult<String> {
        self.enqueued.lock().unwrap().push(result.result_id.clone());
        Ok(format!("t-{}", result.result_id))
    }
    fn status(&self, _: &str) -> AdapterResult<TransferStatus> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(TransferStatus::Completed {
                path: self.file.clone(),
            }))
    }
    fn cancel(&self, _: &str) -> AdapterResult<()> {
        Ok(())
    }
}

fn result(id: &str, path: &str, format: &str, kbps: Option<u32>, secs: u64) -> SearchResult {
    SearchResult {
        result_id: id.into(),
        filename: path.into(),
        size_bytes: 1,
        duration_ms: Some(secs * 1000),
        format: Some(format.into()),
        bitrate_kbps: kbps,
        ..Default::default()
    }
}

struct Rig {
    conn: Connection,
    source: Arc<Scripted>,
    handlers: HashMap<&'static str, Arc<dyn Handler>>,
    scheduler: Scheduler,
    candidate: String,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new(results: Vec<SearchResult>, states: Vec<TransferStatus>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
        let file = dir.path().join("staging/slskd/tone.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures/tone.flac"),
            &file,
        )
        .unwrap();
        let source = Arc::new(Scripted {
            results,
            states: Mutex::new(states.into()),
            enqueued: Mutex::default(),
            file,
        });
        let handler: Arc<dyn Handler> = Arc::new(AcquireHandler {
            acquirers: vec![source.clone()],
            staging_root: dir.path().join("staging"),
            probe: Arc::new(crate::real_probe::RealProbe),
            poll: Duration::from_millis(1),
            queued_watch: Duration::from_millis(20),
        });
        let summary = crate::discovery::ingest(
            &conn,
            "test",
            &[CandidateProposal {
                artist: "Alpha Unit".into(),
                title: "First Light".into(),
                mix: None,
                label: None,
                release: None,
                reasons: vec![],
                evidence: vec![EvidenceProposal {
                    source_kind: "page".into(),
                    source_url: Some("https://example.invalid/list".into()),
                    supplied_text_id: None,
                    excerpt: "Alpha Unit - First Light".into(),
                    confidence: 0.8,
                }],
            }],
            1,
        )
        .unwrap();
        let candidate = summary.created[0].clone();
        crate::discovery::identify(&conn, &candidate, 1).unwrap();
        crate::discovery::queue_acquisition(&conn, &candidate, "slskd", 1).unwrap();
        Rig {
            conn,
            source,
            handlers: HashMap::from([(kinds::ACQUIRE, handler)]),
            scheduler: Scheduler::new(Limits::default(), Arc::new(staged_bytes)),
            candidate,
            _dir: dir,
        }
    }

    fn unattended(self) -> Self {
        crate::settings::set(&self.conn, crate::settings::keys::UNATTENDED_DOWNLOADS, &true).unwrap();
        self
    }

    fn run(&mut self) -> bool {
        self.conn
            .execute("UPDATE job SET next_run_at = 0 WHERE state = 'queued'", [])
            .unwrap();
        run_one(
            &mut self.conn,
            &self.scheduler,
            &self.handlers,
            &[kinds::ACQUIRE],
            "w",
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    fn candidate_state(&self) -> (String, String, Option<String>) {
        self.conn
            .query_row(
                "SELECT stage, status, status_reason FROM candidate WHERE id = ?1",
                [&self.candidate],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }

    fn validations(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM job WHERE kind = 'validate'", [], |r| {
                r.get(0)
            })
            .unwrap()
    }
}

fn good_results() -> Vec<SearchResult> {
    vec![
        result("mp3", "@@a\\Alpha Unit - First Light.mp3", "mp3", Some(320), 300),
        result("flac", "@@b\\Alpha Unit - First Light.flac", "flac", None, 301),
    ]
}

#[test]
fn with_unattended_off_the_user_chooses_even_a_clear_match() {
    let mut rig = Rig::new(good_results(), vec![]);
    rig.run();
    assert!(rig.source.enqueued.lock().unwrap().is_empty());
    let (_, status, reason) = rig.candidate_state();
    assert_eq!(status, "blocked");
    assert!(reason
        .unwrap()
        .starts_with("Choose a download: Unattended downloads are off"));
    let pending = choices(&rig.conn).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].outcome.decision,
        matching::Decision::Choose {
            recommended: Some(0),
            why: "Unattended downloads are off. The recommended copy meets the automatic rule.".into()
        }
    );
    assert_eq!(pending[0].outcome.ranked[0].result.result_id, "flac");

    choose(&rig.conn, &rig.candidate, "mp3", 2).unwrap();
    assert!(choices(&rig.conn).unwrap().is_empty());
    rig.run();
    assert_eq!(*rig.source.enqueued.lock().unwrap(), vec!["mp3"]);
    assert_eq!(rig.candidate_state().0, "validating");
    assert_eq!(rig.validations(), 1);
}

#[test]
fn unattended_downloads_the_best_acceptable_copy() {
    let mut rig = Rig::new(good_results(), vec![]).unattended();
    rig.run();
    assert_eq!(*rig.source.enqueued.lock().unwrap(), vec!["flac"]);
    assert_eq!(rig.validations(), 1);
}

#[test]
fn ambiguous_results_wait_for_the_user_even_when_unattended() {
    let results = vec![
        result("a", "@@a\\Alpha Unit - First Light.flac", "flac", None, 300),
        result("b", "@@b\\Alpha Unit - First Light.flac", "flac", None, 420),
    ];
    let mut rig = Rig::new(results, vec![]).unattended();
    rig.run();
    assert!(rig.source.enqueued.lock().unwrap().is_empty());
    assert!(choices(&rig.conn).unwrap()[0].why.contains("differ in length"));
    decline(&rig.conn, &rig.candidate, 2).unwrap();
    let (_, status, reason) = rig.candidate_state();
    assert_eq!(status, "failed");
    assert!(reason.unwrap().contains("does not affect your ratings"));
    let ratings: i64 = rig
        .conn
        .query_row("SELECT COUNT(*) FROM rating_event", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ratings, 0);
}

#[test]
fn nothing_usable_fails_the_candidate_with_a_reason() {
    let mut rig = Rig::new(
        vec![result("x", "@@x\\Other - Song.flac", "flac", None, 300)],
        vec![],
    );
    rig.run();
    let (_, status, reason) = rig.candidate_state();
    assert_eq!(status, "failed");
    assert!(reason.unwrap().contains("No result names this track"));
}

#[test]
fn a_queued_transfer_hands_the_worker_back_without_using_attempts() {
    let queued = vec![TransferStatus::Queued; 1000];
    let mut rig = Rig::new(good_results(), queued).unattended();
    rig.run();
    let (state, attempts, reason): (String, i64, String) = rig
        .conn
        .query_row(
            "SELECT state, attempts, reason FROM job WHERE kind = 'acquire'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), attempts), ("queued", 0));
    assert!(reason.contains("upload queue"));
    // The next run resumes the same transfer rather than enqueuing again.
    rig.source.states.lock().unwrap().clear();
    rig.run();
    assert_eq!(rig.source.enqueued.lock().unwrap().len(), 1);
    assert_eq!(rig.validations(), 1);
}

#[test]
fn a_failed_source_is_not_tried_again() {
    let failed = vec![TransferStatus::Failed {
        reason: "user offline".into(),
    }];
    let mut rig = Rig::new(good_results(), failed).unattended();
    rig.run();
    rig.run();
    assert_eq!(*rig.source.enqueued.lock().unwrap(), vec!["flac", "mp3"]);
    assert_eq!(rig.validations(), 1);
}
