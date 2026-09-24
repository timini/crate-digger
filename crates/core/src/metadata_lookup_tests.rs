use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::*;
use crate::adapters::AdapterResult;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::run_one;

fn found(score: f64, title: &str, duration_ms: i64) -> Identified {
    Identified {
        score,
        duration_ms: Some(duration_ms),
        fields: vec![
            ("artist".into(), "Kerri Chandler".into()),
            ("title".into(), title.into()),
            ("year".into(), "1995".into()),
        ],
        discogs_fields: vec![("genre".into(), "Deep House".into())],
        external_ids: vec![("musicbrainz_recording".into(), format!("mb-{title}"))],
    }
}

fn setup() -> (Connection, String) {
    let conn = crate::db::open_in_memory().unwrap();
    let t = meta::create_track(&conn).unwrap();
    meta::set_extracted(
        &conn,
        &t,
        "tags",
        &[
            (Field::Artist, Some("kerri chandler".into())),
            (Field::Title, Some("track 01".into())),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO fingerprint (id, track_id, algorithm, duration_ms, data, created_at)
         VALUES ('fp', ?1, 'chromaprint-test2', 300000, ?2, 1)",
        params![t, vec![1u8, 0, 0, 0, 2, 0, 0, 0]],
    )
    .unwrap();
    (conn, t)
}

fn state(conn: &Connection, t: &str) -> String {
    status(conn, t).unwrap().unwrap().status
}

#[test]
fn a_confident_match_replaces_file_tags_but_not_user_edits() {
    let (conn, t) = setup();
    meta::set_correction(&conn, &t, Field::Year, Some("1994")).unwrap();
    record(&conn, &t, 301_000, &[found(0.97, "Rain", 302_000)], 1).unwrap();
    assert_eq!(state(&conn, &t), "identified");
    let m = meta::effective(&conn, &t).unwrap();
    assert_eq!(
        m.title.as_deref(),
        Some("Rain"),
        "identified title beats the file tag"
    );
    assert_eq!(m.artist.as_deref(), Some("Kerri Chandler"));
    assert_eq!(m.genre.as_deref(), Some("Deep House"));
    assert_eq!(m.year, Some(1994), "the user's edit wins");
    let ids: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM track_external_id WHERE track_id = ?1",
            [&t],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ids, 1);
}

#[test]
fn uncertain_matches_are_only_suggested() {
    for results in [
        vec![found(0.8, "Rain", 300_000)],
        vec![found(0.95, "Rain", 360_000)],
        vec![found(0.95, "Rain", 300_000), found(0.93, "Another Song", 300_000)],
    ] {
        let (conn, t) = setup();
        record(&conn, &t, 300_000, &results, 1).unwrap();
        assert_eq!(state(&conn, &t), "suggested");
        assert_eq!(
            meta::effective(&conn, &t).unwrap().title.as_deref(),
            Some("track 01")
        );
        let s = suggestions(&conn).unwrap();
        assert_eq!(s.len(), results.len());

        accept(&conn, &s[0].id, 2).unwrap();
        assert_eq!(state(&conn, &t), "identified");
        assert_eq!(meta::effective(&conn, &t).unwrap().title.as_deref(), Some("Rain"));
        assert!(suggestions(&conn).unwrap().is_empty());
    }
    let (conn, t) = setup();
    record(&conn, &t, 300_000, &[found(0.5, "Rain", 300_000)], 1).unwrap();
    dismiss(&conn, &t, 2).unwrap();
    assert_eq!(
        meta::effective(&conn, &t).unwrap().title.as_deref(),
        Some("track 01")
    );
    record(&conn, &t, 300_000, &[], 3).unwrap();
    assert_eq!(state(&conn, &t), "not_found");
}

struct Fake(Mutex<AdapterResult<Vec<Identified>>>, Mutex<Vec<MetadataQuery>>);

impl MetadataLookup for Fake {
    fn lookup(&self, q: &MetadataQuery) -> AdapterResult<Vec<Identified>> {
        self.1.lock().unwrap().push(q.clone());
        self.0.lock().unwrap().clone()
    }
}

fn run(conn: &mut Connection, lookup: Arc<Fake>) {
    let h: Arc<dyn Handler> = Arc::new(MetadataHandler { lookup });
    let handlers = HashMap::from([(kinds::METADATA, h)]);
    let s = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    run_one(
        conn,
        &s,
        &handlers,
        &[kinds::METADATA],
        "w",
        &AtomicBool::new(false),
    )
    .unwrap();
}

#[test]
fn jobs_wait_for_a_key_then_identify_every_fingerprinted_track() {
    let (mut conn, t) = setup();
    let lookup = Arc::new(Fake(
        Mutex::new(Err(AdapterError::Auth("Add an AcoustID key in Settings.".into()))),
        Mutex::default(),
    ));
    assert_eq!(queue_all(&conn, 1).unwrap(), 1);
    assert_eq!(queue_all(&conn, 1).unwrap(), 0, "already queued");
    run(&mut conn, lookup.clone());
    assert_eq!(state(&conn, &t), "waiting");

    *lookup.0.lock().unwrap() = Ok(vec![found(0.99, "Rain", 300_000)]);
    crate::jobs::set_connector_status(&conn, CONNECTOR, crate::jobs::ConnectorStatus::Ok, None, 2).unwrap();
    run(&mut conn, lookup.clone());
    assert_eq!(state(&conn, &t), "identified");
    let q = &lookup.1.lock().unwrap()[1];
    assert_eq!((q.fingerprint.clone(), q.duration_ms), (vec![1, 2], 300_000));
    assert_eq!(q.title.as_deref(), Some("track 01"));
}
