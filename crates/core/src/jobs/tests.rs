use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::scheduler::{Limits, Scheduler};
use super::worker::{run_one, Handler, JobCtx, WorkerPool};
use super::*;
use crate::adapters::fake::{FakeAcquirer, FakeCentral};
use crate::adapters::{Acquirer, AcquisitionQuery, CentralSync, TransferStatus};

const DAY: i64 = 86_400_000;

struct TestClock(Arc<AtomicI64>);

impl TestClock {
    fn new() -> Self {
        // 2026-09-23 12:00 UTC
        TestClock(Arc::new(AtomicI64::new(1_790_164_800_000)))
    }
    fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn scheduler(limits: Limits, clock: &TestClock, usage: Arc<AtomicI64>) -> Scheduler {
    let c = clock.0.clone();
    Scheduler::with_clock(
        limits,
        Arc::new(move |_: &Connection| usage.load(Ordering::SeqCst) as u64),
        Arc::new(move || c.load(Ordering::SeqCst)),
    )
}

fn default_scheduler(clock: &TestClock) -> Scheduler {
    scheduler(Limits::default(), clock, Arc::new(AtomicI64::new(0)))
}

fn add(conn: &Connection, kind: &str, key: &str, now: i64) -> String {
    enqueue(conn, &NewJob::new(kind, key, serde_json::json!({})), now)
        .unwrap()
        .id
}

fn no_reasonless_holds(conn: &Connection) {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM job WHERE state NOT IN ('queued', 'running', 'done') AND reason IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "a held job has no reason");
}

#[test]
fn enqueue_is_idempotent() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = enqueue(&conn, &NewJob::new("x", "same", serde_json::json!({})), 1).unwrap();
    let b = enqueue(
        &conn,
        &NewJob::new("x", "same", serde_json::json!({"other": 1})),
        2,
    )
    .unwrap();
    assert!(a.created);
    assert!(!b.created);
    assert_eq!(a.id, b.id);
}

#[test]
fn backoff_doubles_and_caps() {
    assert_eq!(backoff_ms(1), 2_000);
    assert_eq!(backoff_ms(2), 4_000);
    assert_eq!(backoff_ms(4), 16_000);
    assert_eq!(backoff_ms(50), 600_000);
}

#[test]
fn concurrency_limit_per_kind() {
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    let mut conn = crate::db::open_in_memory().unwrap();
    for i in 0..3 {
        add(&conn, kinds::ACQUIRE, &format!("a{i}"), clock.now());
    }
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_some());
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_some());
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_none());
}

#[test]
fn analysis_yields_to_playback() {
    let clock = TestClock::new();
    let mut limits = Limits::default();
    limits.concurrency.insert(kinds::ANALYSE.into(), 2);
    let s = scheduler(limits, &clock, Arc::new(AtomicI64::new(0)));
    let mut conn = crate::db::open_in_memory().unwrap();
    for i in 0..3 {
        add(&conn, kinds::ANALYSE, &format!("an{i}"), clock.now());
    }
    s.set_playback_active(true);
    assert!(s.claim(&mut conn, "w", &[kinds::ANALYSE]).unwrap().is_some());
    assert!(s.claim(&mut conn, "w", &[kinds::ANALYSE]).unwrap().is_none());
    s.set_playback_active(false);
    assert!(s.claim(&mut conn, "w", &[kinds::ANALYSE]).unwrap().is_some());
}

#[test]
fn daily_limit_blocks_with_reason_and_resets_next_day() {
    let clock = TestClock::new();
    let mut limits = Limits::default();
    limits.daily.insert(kinds::ACQUIRE.into(), 2);
    limits.concurrency.insert(kinds::ACQUIRE.into(), 10);
    let s = scheduler(limits, &clock, Arc::new(AtomicI64::new(0)));
    let mut conn = crate::db::open_in_memory().unwrap();
    for i in 0..3 {
        add(&conn, kinds::ACQUIRE, &format!("a{i}"), clock.now());
    }
    let first = s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().unwrap();
    s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().unwrap();
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_none());

    let blocked = list(&conn, &[JobState::Blocked], 10).unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].hold_code, Some(HoldCode::DailyLimit));
    assert!(blocked[0].reason.as_ref().unwrap().contains("Daily limit of 2"));
    no_reasonless_holds(&conn);

    // A retry of an already-started job does not count as a new job.
    record_failure(
        &conn,
        &first,
        "w",
        &JobError::Retryable("flaky".into()),
        clock.now(),
    )
    .unwrap();
    clock.advance(60_000);
    let again = s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().unwrap();
    assert_eq!(again.id, first.id);

    clock.advance(DAY);
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_some());
}

#[test]
fn storage_budget_blocks_acquisition_until_space_frees() {
    let clock = TestClock::new();
    let usage = Arc::new(AtomicI64::new(0));
    let limits = Limits {
        staging_budget_bytes: 1_000,
        ..Default::default()
    };
    let s = scheduler(limits, &clock, usage.clone());
    let mut conn = crate::db::open_in_memory().unwrap();
    add(&conn, kinds::ACQUIRE, "a", clock.now());
    add(&conn, kinds::ANALYSE, "b", clock.now());

    usage.store(1_000, Ordering::SeqCst);
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_none());
    let blocked = list(&conn, &[JobState::Blocked], 10).unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].hold_code, Some(HoldCode::StorageLimit));
    assert!(blocked[0].reason.as_ref().unwrap().contains("Temporary audio"));
    // Other kinds keep running.
    assert!(s.claim(&mut conn, "w", &[kinds::ANALYSE]).unwrap().is_some());

    usage.store(10, Ordering::SeqCst);
    assert!(s.claim(&mut conn, "w", &[kinds::ACQUIRE]).unwrap().is_some());
}

#[test]
fn retryable_failure_backs_off_then_gives_up_with_reason() {
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    let mut conn = crate::db::open_in_memory().unwrap();
    let id = enqueue(
        &conn,
        &NewJob {
            max_attempts: 2,
            ..NewJob::new("x", "k", serde_json::json!({}))
        },
        clock.now(),
    )
    .unwrap()
    .id;

    let job = s.claim(&mut conn, "w", &["x"]).unwrap().unwrap();
    let st = record_failure(
        &conn,
        &job,
        "w",
        &JobError::Retryable("timeout".into()),
        clock.now(),
    )
    .unwrap();
    assert_eq!(st, Some(JobState::Queued));
    assert!(get(&conn, &id)
        .unwrap()
        .reason
        .unwrap()
        .contains("Retrying in 2 s"));
    assert!(
        s.claim(&mut conn, "w", &["x"]).unwrap().is_none(),
        "should wait for backoff"
    );

    clock.advance(2_000);
    let job = s.claim(&mut conn, "w", &["x"]).unwrap().unwrap();
    let st = record_failure(
        &conn,
        &job,
        "w",
        &JobError::Retryable("timeout".into()),
        clock.now(),
    )
    .unwrap();
    assert_eq!(st, Some(JobState::Failed));
    assert!(get(&conn, &id)
        .unwrap()
        .reason
        .unwrap()
        .contains("Gave up after 2 attempts"));

    retry(&conn, &id, clock.now()).unwrap();
    assert_eq!(get(&conn, &id).unwrap().attempts, 0);
    assert!(s.claim(&mut conn, "w", &["x"]).unwrap().is_some());
}

#[test]
fn auth_failure_pauses_connector_until_fixed() {
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    let mut conn = crate::db::open_in_memory().unwrap();
    for i in 0..2 {
        enqueue(
            &conn,
            &NewJob::new("search", format!("s{i}"), serde_json::json!({})).connector("slskd"),
            clock.now(),
        )
        .unwrap();
    }
    enqueue(
        &conn,
        &NewJob::new("search", "other", serde_json::json!({})).connector("discogs"),
        clock.now(),
    )
    .unwrap();

    let job = s.claim(&mut conn, "w", &["search"]).unwrap().unwrap();
    let err = JobError::Auth {
        connector: "slskd".into(),
        message: "401 Unauthorized".into(),
    };
    assert_eq!(
        record_failure(&conn, &job, "w", &err, clock.now()).unwrap(),
        Some(JobState::Paused)
    );
    let paused = list(&conn, &[JobState::Paused], 10).unwrap();
    assert_eq!(paused.len(), 2, "every slskd job pauses");
    assert!(paused
        .iter()
        .all(|j| j.hold_code == Some(HoldCode::ConnectorAuth)));
    assert!(paused[0]
        .reason
        .as_ref()
        .unwrap()
        .contains("Fix the connection in Settings"));
    no_reasonless_holds(&conn);

    // Other connectors keep working.
    let other = s.claim(&mut conn, "w", &["search"]).unwrap().unwrap();
    assert_eq!(other.connector.as_deref(), Some("discogs"));
    assert!(s.claim(&mut conn, "w", &["search"]).unwrap().is_none());

    set_connector_status(&conn, "slskd", ConnectorStatus::Ok, None, clock.now()).unwrap();
    assert_eq!(list(&conn, &[JobState::Queued], 10).unwrap().len(), 2);
}

#[test]
fn pause_all_resume_all_and_cancel() {
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    let mut conn = crate::db::open_in_memory().unwrap();
    let a = add(&conn, "x", "a", clock.now());
    let b = add(&conn, "x", "b", clock.now());
    let running = s.claim(&mut conn, "w", &["x"]).unwrap().unwrap();
    assert_eq!(running.id, a);

    assert_eq!(pause_all(&conn, clock.now()).unwrap(), 2);
    no_reasonless_holds(&conn);
    assert!(s.claim(&mut conn, "w", &["x"]).unwrap().is_none());
    // The worker that was running loses its lease.
    assert_eq!(
        complete(&conn, &running.id, "w", clock.now()),
        Err(JobError::Stopped)
    );

    assert_eq!(resume_all(&conn, clock.now()).unwrap(), 2);
    cancel(&conn, &a, clock.now()).unwrap();
    assert_eq!(get(&conn, &a).unwrap().state, JobState::Cancelled);
    assert_eq!(
        get(&conn, &a).unwrap().reason.as_deref(),
        Some("Cancelled by you")
    );
    assert!(
        retry(&conn, &b, clock.now()).is_err(),
        "queued jobs cannot be retried"
    );
    retry(&conn, &a, clock.now()).unwrap();
}

#[test]
fn quit_parks_running_jobs_and_start_resumes_them() {
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    let mut conn = crate::db::open_in_memory().unwrap();
    let id = add(&conn, "x", "a", clock.now());
    s.claim(&mut conn, "w", &["x"]).unwrap().unwrap();
    assert_eq!(park_for_quit(&conn, clock.now()).unwrap(), 1);
    let j = get(&conn, &id).unwrap();
    assert_eq!((j.state, j.hold_code), (JobState::Paused, Some(HoldCode::Quit)));
    assert!(j.reason.unwrap().contains("Resumes on next start"));

    let r = recover_on_start(&conn, clock.now()).unwrap();
    assert_eq!(r.resumed_after_quit, 1);
    assert_eq!(get(&conn, &id).unwrap().state, JobState::Queued);
}

#[test]
fn user_pause_survives_restart() {
    let clock = TestClock::new();
    let conn = crate::db::open_in_memory().unwrap();
    let id = add(&conn, "x", "a", clock.now());
    pause_all(&conn, clock.now()).unwrap();
    recover_on_start(&conn, clock.now()).unwrap();
    assert_eq!(get(&conn, &id).unwrap().state, JobState::Paused);
}

// ---------------------------------------------------------------------------
// Crash injection.
//
// `crash_run` claims a job and runs its handler, and if the handler panics at
// a fail point it drops everything without recording an outcome, exactly as
// if the process had died. The test then "restarts": recover_on_start, run
// the job to completion, and check the side effect happened exactly once.
// ---------------------------------------------------------------------------

fn crash_run(conn: &mut Connection, s: &Scheduler, h: &dyn Handler) -> bool {
    let job = s.claim(conn, "doomed-worker", &[h.kind()]).unwrap().unwrap();
    let stop = AtomicBool::new(false);
    let mut ctx = JobCtx::new(conn, &job, "doomed-worker", s, &stop);
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| h.run(&mut ctx)));
    r.is_err()
}

fn restart_and_finish(conn: &mut Connection, s: &Scheduler, h: Arc<dyn Handler>) {
    let rec = recover_on_start(conn, s.now()).unwrap();
    assert_eq!(rec.resumed_after_crash, 1);
    let handlers: HashMap<&'static str, Arc<dyn Handler>> = HashMap::from([(h.kind(), h.clone())]);
    let stop = AtomicBool::new(false);
    assert!(run_one(conn, s, &handlers, &[h.kind()], "new-worker", &stop).unwrap());
    let done: i64 = conn
        .query_row("SELECT COUNT(*) FROM job WHERE state = 'done'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(done, 1);
}

#[derive(Serialize, Deserialize)]
struct TransferCheckpoint {
    transfer_id: String,
}

/// Mirrors the real acquisition handler: start (idempotently), checkpoint
/// the transfer, wait for completion, register the staged file once.
struct TransferHandler {
    acquirer: Arc<FakeAcquirer>,
    staging: std::path::PathBuf,
}

impl Handler for TransferHandler {
    fn kind(&self) -> &'static str {
        kinds::ACQUIRE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let transfer_id = match ctx.checkpoint::<TransferCheckpoint>() {
            Some(c) => c.transfer_id,
            None => {
                let q = AcquisitionQuery {
                    artist: "A".into(),
                    title: "B".into(),
                    mix: None,
                };
                let r = self
                    .acquirer
                    .search(&q)
                    .map_err(|e| ctx.adapter_error(e))?
                    .remove(0);
                let id = self
                    .acquirer
                    .enqueue(&r, &ctx.job.idempotency_key, &self.staging)
                    .map_err(|e| ctx.adapter_error(e))?;
                fail::fail_point!("transfer.before_checkpoint");
                ctx.save_checkpoint(&TransferCheckpoint {
                    transfer_id: id.clone(),
                })?;
                id
            }
        };
        let TransferStatus::Completed { path } = self
            .acquirer
            .status(&transfer_id)
            .map_err(|e| ctx.adapter_error(e))?
        else {
            return Err(JobError::Retryable("transfer not finished".into()));
        };
        fail::fail_point!("transfer.after_complete");
        ctx.conn
            .execute(
                "INSERT OR IGNORE INTO effect (key, detail) VALUES (?1, ?2)",
                params![ctx.job.idempotency_key, path.display().to_string()],
            )
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        fail::fail_point!("transfer.after_register");
        Ok(())
    }
}

fn effect_table(conn: &Connection) {
    conn.execute_batch("CREATE TABLE effect (key TEXT PRIMARY KEY, detail TEXT)")
        .unwrap();
}

fn transfer_crash_case(point: &str) {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("fixture.wav");
    std::fs::write(&src, b"RIFFfake").unwrap();
    let acquirer = Arc::new(FakeAcquirer::new(vec![src]));
    let staging = dir.path().join("staging");
    let handler = Arc::new(TransferHandler {
        acquirer: acquirer.clone(),
        staging: staging.clone(),
    });

    let db_path = dir.path().join("db.sqlite");
    let mut conn = crate::db::open(&db_path).unwrap();
    effect_table(&conn);
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    add(&conn, kinds::ACQUIRE, "acquire:cand-1", clock.now());

    fail::cfg(point, "panic").unwrap();
    assert!(crash_run(&mut conn, &s, handler.as_ref()), "{point} did not fire");
    fail::remove(point);

    // Simulate a process restart with a fresh connection.
    drop(conn);
    let mut conn = crate::db::open(&db_path).unwrap();
    restart_and_finish(&mut conn, &s, handler);

    assert_eq!(
        acquirer.started.load(Ordering::SeqCst),
        1,
        "transfer started twice"
    );
    let effects: i64 = conn
        .query_row("SELECT COUNT(*) FROM effect", [], |r| r.get(0))
        .unwrap();
    assert_eq!(effects, 1);
    assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 1);
    scenario.teardown();
}

#[test]
fn crash_after_transfer_starts_before_checkpoint() {
    transfer_crash_case("transfer.before_checkpoint");
}

#[test]
fn crash_at_transfer_completion() {
    transfer_crash_case("transfer.after_complete");
}

#[test]
fn crash_after_transfer_registered() {
    transfer_crash_case("transfer.after_register");
}

/// Mirrors analysis: write a feature record keyed by source fingerprint and
/// model, so a rerun finds the existing record instead of adding another.
struct AnalysisHandler {
    track_id: String,
}

impl Handler for AnalysisHandler {
    fn kind(&self) -> &'static str {
        kinds::ANALYSE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let exists: i64 = ctx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM feature_record
                 WHERE track_id = ?1 AND model_id = 'test-model' AND source_fingerprint = 'fp-1'",
                params![self.track_id],
                |r| r.get(0),
            )
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        if exists == 0 {
            ctx.conn
                .execute(
                    "INSERT INTO feature_record (id, track_id, model_id, weights_checksum,
                         preprocessing_version, source_fingerprint, segment_start_ms,
                         segment_end_ms, dims, created_at)
                     VALUES (?1, ?2, 'test-model', 'sha256:0', 'v1', 'fp-1', 0, 30000, 0, 0)",
                    params![crate::util::new_id(), self.track_id],
                )
                .map_err(|e| JobError::Retryable(e.to_string()))?;
        }
        fail::fail_point!("analysis.after_complete");
        Ok(())
    }
}

#[test]
fn crash_at_analysis_completion() {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db.sqlite");
    let mut conn = crate::db::open(&db_path).unwrap();
    let track_id = crate::meta::create_track(&conn).unwrap();
    let handler = Arc::new(AnalysisHandler { track_id });
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    add(&conn, kinds::ANALYSE, "analyse:t1", clock.now());

    fail::cfg("analysis.after_complete", "panic").unwrap();
    assert!(crash_run(&mut conn, &s, handler.as_ref()));
    fail::remove("analysis.after_complete");

    drop(conn);
    let mut conn = crate::db::open(&db_path).unwrap();
    restart_and_finish(&mut conn, &s, handler);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM feature_record", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    scenario.teardown();
}

/// Mirrors outbox sync: submit with the outbox idempotency key, then mark
/// acknowledged. A crash between the two must not create a second
/// contribution on the service.
struct SyncHandler {
    central: Arc<FakeCentral>,
}

impl Handler for SyncHandler {
    fn kind(&self) -> &'static str {
        kinds::SYNC
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let (key, payload): (String, String) = ctx
            .conn
            .query_row(
                "SELECT idempotency_key, payload FROM sync_outbox WHERE state = 'pending' LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        self.central
            .submit(&key, "metadata", &payload)
            .map_err(|e| ctx.adapter_error(e))?;
        fail::fail_point!("sync.after_submit");
        ctx.conn
            .execute(
                "UPDATE sync_outbox SET state = 'acked', acked_at = ?2 WHERE idempotency_key = ?1",
                params![key, ctx.now()],
            )
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        Ok(())
    }
}

#[test]
fn crash_at_sync_acknowledgement() {
    let scenario = fail::FailScenario::setup();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db.sqlite");
    let mut conn = crate::db::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO sync_outbox (id, idempotency_key, kind, payload, state, created_at)
         VALUES ('o1', 'contrib-1', 'metadata', '{\"title\":\"x\"}', 'pending', 0)",
        [],
    )
    .unwrap();
    let central = Arc::new(FakeCentral::default());
    let handler = Arc::new(SyncHandler {
        central: central.clone(),
    });
    let clock = TestClock::new();
    let s = default_scheduler(&clock);
    add(&conn, kinds::SYNC, "sync:o1", clock.now());

    fail::cfg("sync.after_submit", "panic").unwrap();
    assert!(crash_run(&mut conn, &s, handler.as_ref()));
    fail::remove("sync.after_submit");
    let pending: String = conn
        .query_row("SELECT state FROM sync_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, "pending", "unacknowledged item must be kept");

    drop(conn);
    let mut conn = crate::db::open(&db_path).unwrap();
    restart_and_finish(&mut conn, &s, handler);
    assert_eq!(central.accepted.lock().unwrap().len(), 1);
    let state: String = conn
        .query_row("SELECT state FROM sync_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "acked");
    scenario.teardown();
}

// ---------------------------------------------------------------------------
// Worker pool.
// ---------------------------------------------------------------------------

struct CountingHandler;

impl Handler for CountingHandler {
    fn kind(&self) -> &'static str {
        "count"
    }
    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        ctx.conn
            .execute(
                "INSERT INTO effect (key) VALUES (?1)",
                params![ctx.job.idempotency_key],
            )
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        Ok(())
    }
}

/// Runs until told to stop, checkpointing as it goes.
struct LongHandler;

impl Handler for LongHandler {
    fn kind(&self) -> &'static str {
        "long"
    }
    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        for i in 0..10_000u32 {
            ctx.save_checkpoint(&i)?;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }
}

fn wait_for(mut cond: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(start.elapsed().as_secs() < 10, "timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn worker_pool_runs_jobs_and_stops_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db.sqlite");
    let conn = crate::db::open(&db_path).unwrap();
    effect_table(&conn);
    let s = Arc::new(Scheduler::new(Limits::default(), Arc::new(|_: &Connection| 0)));
    for i in 0..20 {
        add(&conn, "count", &format!("c{i}"), crate::util::now_ms());
    }
    add(&conn, "long", "long-1", crate::util::now_ms());

    let pool = WorkerPool::start(
        db_path.clone(),
        s.clone(),
        vec![Arc::new(CountingHandler), Arc::new(LongHandler)],
        2,
    )
    .unwrap();
    pool.notify();
    wait_for(|| {
        conn.query_row("SELECT COUNT(*) FROM effect", [], |r| r.get::<_, i64>(0))
            .unwrap()
            == 20
    });
    wait_for(|| {
        conn.query_row(
            "SELECT checkpoint IS NOT NULL FROM job WHERE kind = 'long'",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    });

    // Quit: stop workers, then park what was running.
    s.stop_accepting();
    pool.stop();
    assert_eq!(park_for_quit(&conn, crate::util::now_ms()).unwrap(), 1);
    let long: (String, String) = conn
        .query_row("SELECT state, hold_code FROM job WHERE kind = 'long'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(long, ("paused".to_string(), "quit".to_string()));
    no_reasonless_holds(&conn);
}

struct WaitingHandler;

impl Handler for WaitingHandler {
    fn kind(&self) -> &'static str {
        "waiting"
    }
    fn run(&self, _: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        Err(JobError::Wait {
            reason: "Queued on the uploader's side.".into(),
            delay_ms: 60_000,
        })
    }
}

#[test]
fn waiting_reschedules_without_using_attempts() {
    let mut conn = crate::db::open_in_memory().unwrap();
    let clock = TestClock::new();
    let sched = default_scheduler(&clock);
    let handlers: HashMap<&'static str, Arc<dyn Handler>> =
        HashMap::from([("waiting", Arc::new(WaitingHandler) as Arc<dyn Handler>)]);
    let id = add(&conn, "waiting", "w", clock.now());
    let stop = AtomicBool::new(false);
    for _ in 0..10 {
        assert!(run_one(&mut conn, &sched, &handlers, &["waiting"], "w", &stop).unwrap());
        // Not due again until the delay has passed.
        assert!(!run_one(&mut conn, &sched, &handlers, &["waiting"], "w", &stop).unwrap());
        clock.advance(60_000);
    }
    let job = get(&conn, &id).unwrap();
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.attempts, 0);
    assert_eq!(job.reason.as_deref(), Some("Queued on the uploader's side."));
}
