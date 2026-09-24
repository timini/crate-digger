use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use super::*;
use crate::adapters::fake::FakeCentral;
use crate::adapters::AdapterResult;
use crate::domain::Field;
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::run_one;

fn identified(conn: &Connection) -> String {
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "musicbrainz",
        &[
            (Field::Artist, Some("Kerri Chandler".into())),
            (Field::Title, Some("Rain".into())),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO track_external_id (track_id, namespace, value, source) VALUES (?1, 'musicbrainz_recording', 'rec-1', 'acoustid'),
                (?1, 'acoustid', 'aid-1', 'acoustid')",
        params![t],
    )
    .unwrap();
    t
}

fn add_embedding(conn: &Connection, t: &str) {
    let v = shared_version();
    let bytes: Vec<u8> = vec![0.25f32; 1280].iter().flat_map(|x| x.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version, source_fingerprint,
             segment_start_ms, segment_end_ms, dims, embedding, tempo, created_at)
         VALUES ('fr', ?1, ?2, ?3, ?4, 'fp', 0, 1, 1280, ?5, 124.0, 1)",
        params![t, v.model_id, v.weights_checksum, v.preprocessing_version, bytes],
    )
    .unwrap();
}

#[test]
fn only_identified_tracks_are_shared_and_never_private_data() {
    let conn = crate::db::open_in_memory().unwrap();
    let plain = meta::create_track(&conn).unwrap();
    assert!(contribution(&conn, &plain).unwrap().is_none());

    let t = identified(&conn);
    crate::review::rate(&conn, &t, crate::domain::RatingKind::Star3, "s", 1).unwrap();
    conn.execute(
        "INSERT INTO youtube_match (id, track_id, video_id, url, looked_up_at, confidence, preferred)
         VALUES ('y', ?1, 'dQw4w9WgXcQ', 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', 1, 0.9, 1)",
        params![t],
    )
    .unwrap();
    add_embedding(&conn, &t);
    let c = contribution(&conn, &t).unwrap().unwrap();
    assert_eq!(c.recording.external_ids.len(), 1, "only shareable id kinds");
    assert_eq!(c.references.len(), 1);
    assert_eq!(c.features.as_ref().unwrap().embedding.len(), 1280);
    let json = serde_json::to_string(&c).unwrap();
    for private in ["star3", "rating", "path", "/Users", "session"] {
        assert!(!json.contains(private), "{private} leaked: {json}");
    }
}

#[test]
fn the_outbox_sends_each_version_once_and_only_while_sharing_is_on() {
    let conn = crate::db::open_in_memory().unwrap();
    let t = identified(&conn);
    assert!(!queue_share(&conn, &t, 1).unwrap(), "off by default");
    settings::set(&conn, settings::keys::SHARING, &true).unwrap();
    assert!(queue_share(&conn, &t, 1).unwrap());
    assert!(!queue_share(&conn, &t, 2).unwrap(), "same content, same key");
    meta::set_correction(&conn, &t, Field::Year, Some("1995")).unwrap();
    assert!(
        queue_share(&conn, &t, 3).unwrap(),
        "changed content is a new contribution"
    );
    assert_eq!(outbox(&conn).unwrap().pending, 2);
}

struct Scripted(Mutex<Vec<AdapterResult<()>>>, FakeCentral);

impl CentralSync for Scripted {
    fn submit(
        &self,
        key: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> AdapterResult<crate::adapters::SyncAck> {
        if let Some(r) = self.0.lock().unwrap().pop() {
            r?;
        }
        self.1.submit(key, kind, payload)
    }
}

fn run(conn: &mut Connection, h: Arc<dyn Handler>, kind: &'static str) {
    let handlers = HashMap::from([(kind, h)]);
    let s = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    conn.execute("UPDATE job SET next_run_at = 0", []).unwrap();
    run_one(conn, &s, &handlers, &[kind], "w", &AtomicBool::new(false)).unwrap();
}

#[test]
fn outages_wait_rejections_stop_and_acks_are_recorded() {
    let mut conn = crate::db::open_in_memory().unwrap();
    settings::set(&conn, settings::keys::SHARING, &true).unwrap();
    let t = identified(&conn);
    queue_share(&conn, &t, 1).unwrap();
    // Newest first: an outage, then success.
    let central = Arc::new(Scripted(
        Mutex::new(vec![Ok(()), Err(AdapterError::Unavailable("down".into()))]),
        FakeCentral::default(),
    ));
    run(
        &mut conn,
        Arc::new(ShareHandler {
            central: central.clone(),
        }),
        kinds::SYNC,
    );
    let (state, attempts): (String, i64) = conn
        .query_row("SELECT state, attempts FROM job WHERE kind = 'sync'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        (state.as_str(), attempts),
        ("queued", 0),
        "an outage waits without using attempts"
    );
    assert_eq!(outbox(&conn).unwrap().pending, 1);

    run(
        &mut conn,
        Arc::new(ShareHandler {
            central: central.clone(),
        }),
        kinds::SYNC,
    );
    assert_eq!(outbox(&conn).unwrap().acked, 1);
    assert_eq!(central.1.accepted.lock().unwrap().len(), 1);

    // A contribution the service refuses is not retried forever.
    meta::set_correction(&conn, &t, Field::Year, Some("1995")).unwrap();
    queue_share(&conn, &t, 2).unwrap();
    let refusing = Arc::new(Scripted(
        Mutex::new(vec![Err(AdapterError::Invalid("wrong_dimensions".into()))]),
        FakeCentral::default(),
    ));
    run(
        &mut conn,
        Arc::new(ShareHandler { central: refusing }),
        kinds::SYNC,
    );
    assert_eq!(outbox(&conn).unwrap().rejected, 1);

    // Turning sharing off holds what is left.
    meta::set_correction(&conn, &t, Field::Year, Some("1996")).unwrap();
    queue_share(&conn, &t, 3).unwrap();
    settings::set(&conn, settings::keys::SHARING, &false).unwrap();
    run(
        &mut conn,
        Arc::new(ShareHandler {
            central: central.clone(),
        }),
        kinds::SYNC,
    );
    assert_eq!(outbox(&conn).unwrap().pending, 1);
    assert_eq!(central.1.accepted.lock().unwrap().len(), 1);
}

struct Catalogue {
    entry: Option<cd_protocol::CatalogueEntry>,
    agreeing: u32,
    calls: Mutex<usize>,
}

impl CentralLookup for Catalogue {
    fn lookup(&self, _: &[RecordingKey]) -> AdapterResult<Vec<Option<cd_protocol::CatalogueEntry>>> {
        *self.calls.lock().unwrap() += 1;
        Ok(vec![self.entry.clone()])
    }
    fn features(&self, _: &str, version: &FeatureVersion) -> AdapterResult<cd_protocol::FeaturesResponse> {
        Ok(cd_protocol::FeaturesResponse {
            features: Some(Features {
                version: version.clone(),
                embedding: vec![0.5; 1280],
                tempo_bpm: Some(122.0),
                key_camelot: Some("8A".into()),
                loudness_lufs: None,
            }),
            agreeing_contributors: self.agreeing,
        })
    }
}

fn catalogue(agreeing: u32) -> Arc<Catalogue> {
    let v = shared_version();
    Arc::new(Catalogue {
        entry: Some(cd_protocol::CatalogueEntry {
            recording_id: "r-1".into(),
            metadata: Metadata::default(),
            alternatives: vec![],
            feature_versions: vec![FeatureVersion {
                model_id: v.model_id,
                weights_checksum: v.weights_checksum,
                preprocessing_version: v.preprocessing_version,
            }],
            references: vec![],
        }),
        agreeing,
        calls: Mutex::default(),
    })
}

#[test]
fn shared_features_are_reused_only_when_contributors_agree() {
    for (agreeing, expect) in [(1, false), (2, true)] {
        let mut conn = crate::db::open_in_memory().unwrap();
        let t = identified(&conn);
        after_identified(&conn, &t, 1).unwrap();
        let c = catalogue(agreeing);
        run(
            &mut conn,
            Arc::new(ReuseHandler {
                central: Some(c.clone()),
            }),
            kinds::FEATURE_REUSE,
        );
        assert_eq!(
            summary_embedding(&conn, &t, &shared_version()).unwrap().is_some(),
            expect
        );
    }
    // A track with its own embedding never asks.
    let mut conn = crate::db::open_in_memory().unwrap();
    let t = identified(&conn);
    add_embedding(&conn, &t);
    after_identified(&conn, &t, 1).unwrap();
    let c = catalogue(5);
    run(
        &mut conn,
        Arc::new(ReuseHandler {
            central: Some(c.clone()),
        }),
        kinds::FEATURE_REUSE,
    );
    assert_eq!(*c.calls.lock().unwrap(), 0);
    // Not signed in: nothing happens.
    let mut conn = crate::db::open_in_memory().unwrap();
    let t = identified(&conn);
    after_identified(&conn, &t, 1).unwrap();
    run(
        &mut conn,
        Arc::new(ReuseHandler { central: None }),
        kinds::FEATURE_REUSE,
    );
    assert!(summary_embedding(&conn, &t, &shared_version()).unwrap().is_none());
}
