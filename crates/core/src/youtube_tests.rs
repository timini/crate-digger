use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::*;
use crate::adapters::AdapterResult;
use crate::domain::Field;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::run_one;

fn video(id: &str, confidence: f64) -> VideoMatch {
    VideoMatch {
        video_id: id.into(),
        url: format!("https://www.youtube.com/watch?v={id}"),
        title: Some(format!("Video {id}")),
        channel: Some("Channel".into()),
        duration_ms: Some(300_000),
        confidence,
    }
}

fn setup() -> (tempfile::TempDir, Connection, String) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
    let track = meta::create_track(&conn).unwrap();
    meta::set_extracted(
        &conn,
        &track,
        "test",
        &[
            (Field::Artist, Some("Alpha Unit".into())),
            (Field::Title, Some("First Light".into())),
        ],
    )
    .unwrap();
    (dir, conn, track)
}

fn links(conn: &Connection, track: &str) -> Vec<(String, bool, bool, bool)> {
    conn.prepare(
        "SELECT video_id, preferred, user_corrected, rejected FROM youtube_match
         WHERE track_id = ?1 ORDER BY video_id",
    )
    .unwrap()
    .query_map([track], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
    .unwrap()
    .collect::<std::result::Result<_, _>>()
    .unwrap()
}

fn status_of(conn: &Connection, track: &str) -> String {
    status(conn, track).unwrap().unwrap().status
}

#[test]
fn only_a_confident_match_becomes_preferred() {
    let (_d, conn, track) = setup();
    save_results(
        &conn,
        &track,
        &[video("a", 0.9), video("b", 0.75), video("c", 0.5)],
        1,
    )
    .unwrap();
    let got = links(&conn, &track);
    assert_eq!(
        got.iter()
            .filter(|l| l.1)
            .map(|l| l.0.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    assert_eq!(status_of(&conn, &track), "found");

    save_results(&conn, &track, &[video("c", 0.5)], 2).unwrap();
    assert_eq!(links(&conn, &track), vec![("c".into(), false, false, false)]);
    assert_eq!(status_of(&conn, &track), "uncertain");

    save_results(&conn, &track, &[], 3).unwrap();
    assert!(links(&conn, &track).is_empty());
    assert_eq!(status_of(&conn, &track), "none");
}

#[test]
fn user_choices_survive_refreshes() {
    let (_d, conn, track) = setup();
    save_results(
        &conn,
        &track,
        &[video("auto", 0.9), video("alt", 0.6), video("bad", 0.8)],
        1,
    )
    .unwrap();
    correct(&conn, &track, &video("mine", 0.0), 2).unwrap();
    reject(&conn, &track, "bad", 3).unwrap();
    // A later lookup finds the rejected video again and a new confident one.
    save_results(&conn, &track, &[video("bad", 0.95), video("new", 0.9)], 4).unwrap();
    assert_eq!(
        links(&conn, &track),
        vec![
            ("bad".into(), false, true, true),
            ("mine".into(), true, true, false),
            ("new".into(), false, false, false),
        ]
    );
    assert_eq!(status_of(&conn, &track), "found");

    prefer(&conn, &track, "new", 5).unwrap();
    save_results(&conn, &track, &[video("other", 0.99)], 6).unwrap();
    let preferred: Vec<String> = links(&conn, &track)
        .into_iter()
        .filter(|l| l.1)
        .map(|l| l.0)
        .collect();
    assert_eq!(preferred, vec!["new"]);
    assert!(prefer(&conn, &track, "missing", 7).is_err());
}

#[test]
fn rejecting_the_only_link_leaves_the_track_unresolved() {
    let (_d, conn, track) = setup();
    save_results(&conn, &track, &[video("a", 0.9)], 1).unwrap();
    reject(&conn, &track, "a", 2).unwrap();
    assert_eq!(status_of(&conn, &track), "none");
    let card_links: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM youtube_match WHERE track_id = ?1 AND rejected = 0",
            [&track],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(card_links, 0);
}

struct FakeLookup {
    result: Mutex<AdapterResult<Vec<VideoMatch>>>,
    queries: Mutex<Vec<VideoQuery>>,
}

impl VideoLookup for FakeLookup {
    fn id(&self) -> &str {
        "youtube"
    }
    fn lookup(&self, query: &VideoQuery) -> AdapterResult<Vec<VideoMatch>> {
        self.queries.lock().unwrap().push(query.clone());
        self.result.lock().unwrap().clone()
    }
}

fn run_lookup(conn: &mut Connection, lookup: Arc<FakeLookup>) {
    let handler: Arc<dyn Handler> = Arc::new(YoutubeHandler { lookup });
    let handlers = HashMap::from([(kinds::YOUTUBE, handler)]);
    let scheduler = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    run_one(
        conn,
        &scheduler,
        &handlers,
        &[kinds::YOUTUBE],
        "w",
        &AtomicBool::new(false),
    )
    .unwrap();
}

#[test]
fn lookup_job_stores_results_or_waits_for_a_key() {
    let (_d, mut conn, track) = setup();
    let lookup = Arc::new(FakeLookup {
        result: Mutex::new(Err(AdapterError::Auth(
            "Add a YouTube Data API key in Settings.".into(),
        ))),
        queries: Mutex::default(),
    });
    queue_lookup(&conn, &track, false, 1).unwrap();
    assert_eq!(status_of(&conn, &track), "queued");
    run_lookup(&mut conn, lookup.clone());
    let s = status(&conn, &track).unwrap().unwrap();
    assert_eq!(s.status, "waiting");
    assert!(s.detail.unwrap().contains("YouTube Data API key"));
    let state: String = conn
        .query_row("SELECT state FROM job WHERE kind = 'youtube'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "paused");

    // Queuing the same lookup again does not add a job; a refresh does.
    queue_lookup(&conn, &track, false, 2).unwrap();
    assert_eq!(status_of(&conn, &track), "waiting");
    *lookup.result.lock().unwrap() = Ok(vec![video("a", 0.8)]);
    crate::jobs::set_connector_status(&conn, "youtube", crate::jobs::ConnectorStatus::Ok, None, 3).unwrap();
    run_lookup(&mut conn, lookup.clone());
    assert_eq!(status_of(&conn, &track), "found");
    let q = &lookup.queries.lock().unwrap()[1];
    assert_eq!(
        (q.artist.as_str(), q.title.as_str(), q.mix.as_deref()),
        ("Alpha Unit", "First Light", None)
    );
}
