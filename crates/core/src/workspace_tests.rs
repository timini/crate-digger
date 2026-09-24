use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::*;
use crate::adapters::fake::FakeSource;
use crate::adapters::{CandidateProposal, EvidenceProposal};
use crate::discovery::DiscoverHandler;
use crate::domain::{RatingKind, SeedKind};
use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use crate::jobs::worker::{run_one, Handler};
use crate::{meta, review};

fn version() -> FeatureVersion {
    FeatureVersion {
        model_id: "m".into(),
        weights_checksum: "sha256:0".into(),
        preprocessing_version: "p1".into(),
    }
}

fn proposal(artist: &str, title: &str) -> CandidateProposal {
    CandidateProposal {
        artist: artist.into(),
        title: title.into(),
        mix: None,
        label: Some("Fixture Label".into()),
        release: None,
        reasons: vec!["Found by a fixture".into()],
        evidence: vec![EvidenceProposal {
            source_kind: "discogs".into(),
            source_url: Some(format!("https://example.test/{artist}")),
            supplied_text_id: None,
            excerpt: format!("{artist} - {title}"),
            confidence: 0.8,
        }],
    }
}

fn seed(kind: SeedKind, value: &str) -> Seed {
    Seed {
        kind,
        value: value.into(),
    }
}

struct Rig {
    conn: Connection,
    source: Arc<FakeSource>,
    scheduler: Scheduler,
    handlers: HashMap<&'static str, Arc<dyn Handler>>,
    path: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new(catalogue: Vec<CandidateProposal>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let conn = crate::db::open(&path).unwrap();
        let source = Arc::new(FakeSource::new(catalogue));
        let handler: Arc<dyn Handler> = Arc::new(DiscoverHandler::new(source.clone(), "fake_acquirer"));
        Rig {
            conn,
            source,
            scheduler: Scheduler::new(Limits::default(), Arc::new(staged_bytes)),
            handlers: HashMap::from([(kinds::DISCOVER, handler)]),
            path,
            _dir: dir,
        }
    }

    fn discover_for(&mut self, playlist: &str, limit: usize) {
        request_discovery(&self.conn, "fake_source", playlist, limit, 0).unwrap();
        let stop = AtomicBool::new(false);
        run_one(
            &mut self.conn,
            &self.scheduler,
            &self.handlers,
            &[kinds::DISCOVER],
            "w",
            &stop,
        )
        .unwrap();
    }

    /// Make every candidate ready to review, with local audio and an
    /// embedding on the given axis.
    fn make_ready(&self, axis_of: impl Fn(&str) -> usize) {
        let rows: Vec<(String, String, String)> = self
            .conn
            .prepare("SELECT c.id, c.track_id, m.title FROM candidate c JOIN track_meta m ON m.track_id = c.track_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        for (c, t, title) in rows {
            self.conn
                .execute("UPDATE candidate SET stage = 'ready' WHERE id = ?1", params![c])
                .unwrap();
            self.conn
                .execute(
                    "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash,
                         availability, is_primary, last_checked_at, created_at)
                     VALUES (?1, ?2, ?3, 'staged', 1, 0, ?1, 'available', 1, 0, 0)",
                    params![new_id(), t, format!("/staging/{title}.flac")],
                )
                .unwrap();
            embed(&self.conn, &t, axis_of(&title));
        }
    }

    fn track(&self, title: &str) -> String {
        self.conn
            .query_row(
                "SELECT track_id FROM track_meta WHERE title = ?1",
                params![title],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn queue_titles(&self, playlist: &str) -> Vec<String> {
        queue(&self.conn, playlist, &version(), 50)
            .unwrap()
            .into_iter()
            .map(|s| {
                self.conn
                    .query_row(
                        "SELECT title FROM track_meta WHERE track_id = ?1",
                        params![s.track_id],
                        |r| r.get(0),
                    )
                    .unwrap()
            })
            .collect()
    }
}

fn embed(conn: &Connection, track: &str, axis: usize) {
    let mut v = [0.05f32; 8];
    v[axis] = 1.0;
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    let fv = version();
    conn.execute(
        "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version, source_fingerprint,
             segment_start_ms, segment_end_ms, dims, embedding, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'fp', 0, 1, 8, ?6, 0)",
        params![new_id(), track, fv.model_id, fv.weights_checksum, fv.preprocessing_version, bytes],
    )
    .unwrap();
}

#[test]
fn each_playlist_discovers_from_its_own_brief_and_seeds() {
    let mut rig = Rig::new(vec![
        proposal("Dub Artist", "Deep One"),
        proposal("Dub Artist", "Deep Two"),
        proposal("Peak Artist", "Loud One"),
        proposal("Peak Artist", "Loud Two"),
    ]);
    let warmup = playlists::create(&rig.conn, "Warm-up").unwrap();
    let peak = playlists::create(&rig.conn, "Peak time").unwrap();
    set_brief(&rig.conn, &warmup, "Dubby, spacious, restrained vocals").unwrap();
    save_seeds(&rig.conn, &warmup, &[seed(SeedKind::Label, "Echocord")], 0).unwrap();
    save_seeds(&rig.conn, &peak, &[seed(SeedKind::Artist, "Peak Artist")], 0).unwrap();
    // Global seeds are not used for a playlist run.
    rig.conn
        .execute(
            "INSERT INTO seed (id, kind, value, created_at) VALUES ('s', 'artist', 'Global Only', 0)",
            [],
        )
        .unwrap();

    rig.discover_for(&warmup, 2);
    rig.discover_for(&peak, 2);
    let requests = rig.source.requests.lock().unwrap().clone();
    assert_eq!(
        requests[0].brief.as_deref(),
        Some("Dubby, spacious, restrained vocals")
    );
    assert_eq!(requests[0].seeds, vec![seed(SeedKind::Label, "Echocord")]);
    assert_eq!(requests[1].brief, None);
    assert_eq!(requests[1].seeds, vec![seed(SeedKind::Artist, "Peak Artist")]);

    rig.make_ready(|_| 0);
    assert_eq!(rig.queue_titles(&warmup).len(), 2);
    let mut peak_titles = rig.queue_titles(&peak);
    peak_titles.sort();
    assert_eq!(peak_titles, vec!["Loud One", "Loud Two"]);
    let runs: i64 = rig
        .conn
        .query_row(
            "SELECT COUNT(*) FROM source_run WHERE playlist_id = ?1",
            params![warmup],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(runs, 1);
}

#[test]
fn feedback_is_per_playlist_and_leaves_ratings_alone() {
    let mut rig = Rig::new(vec![proposal("A", "Shared"), proposal("B", "Other")]);
    let warmup = playlists::create(&rig.conn, "Warm-up").unwrap();
    let peak = playlists::create(&rig.conn, "Peak time").unwrap();
    rig.discover_for(&warmup, 2);
    // The same tracks suggested again for another playlist: no new candidates.
    let source = rig.source.clone();
    source.reset();
    rig.discover_for(&peak, 2);
    assert_eq!(
        rig.conn
            .query_row("SELECT COUNT(*) FROM candidate", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    rig.make_ready(|_| 0);
    let shared = rig.track("Shared");
    review::rate(&rig.conn, &shared, RatingKind::Star2, "s", 1).unwrap();

    give_feedback(&rig.conn, &peak, &shared, Verdict::NotForThis, 2).unwrap();
    assert_eq!(rig.queue_titles(&peak), vec!["Other"]);
    assert!(rig.queue_titles(&warmup).contains(&"Shared".to_string()));
    assert_eq!(
        review::effective_rating(&rig.conn, &shared).unwrap(),
        Some(RatingKind::Star2)
    );

    give_feedback(&rig.conn, &warmup, &shared, Verdict::Fits, 3).unwrap();
    assert_eq!(
        playlists::track_ids(&rig.conn, &warmup).unwrap(),
        vec![shared.clone()]
    );
    assert!(playlists::track_ids(&rig.conn, &peak).unwrap().is_empty());
    assert_eq!(
        review::effective_rating(&rig.conn, &shared).unwrap(),
        Some(RatingKind::Star2)
    );

    // Undo in the peak playlist brings the track back to its queue only.
    assert_eq!(undo_feedback(&rig.conn, &peak, 4).unwrap(), Some(shared.clone()));
    assert!(rig.queue_titles(&peak).contains(&"Shared".to_string()));
    assert_eq!(
        playlists::track_ids(&rig.conn, &warmup).unwrap(),
        vec![shared.clone()]
    );

    // A global thumbs down removes it from every playlist queue.
    review::rate(&rig.conn, &shared, RatingKind::ThumbsDown, "s", 5).unwrap();
    assert!(!rig.queue_titles(&peak).contains(&"Shared".to_string()));
}

#[test]
fn accepting_appends_and_undo_takes_only_that_entry_out() {
    let mut rig = Rig::new(vec![proposal("A", "New")]);
    let list = playlists::create(&rig.conn, "Set").unwrap();
    rig.discover_for(&list, 1);
    rig.make_ready(|_| 0);
    let existing = meta::create_track(&rig.conn).unwrap();
    playlists::add_tracks(&rig.conn, &list, std::slice::from_ref(&existing), None).unwrap();
    let new = rig.track("New");

    give_feedback(&rig.conn, &list, &new, Verdict::Fits, 1).unwrap();
    assert_eq!(
        playlists::track_ids(&rig.conn, &list).unwrap(),
        vec![existing.clone(), new.clone()]
    );
    // The user moves it to the top; undo still removes just that entry.
    playlists::move_entry(&rig.conn, &list, 1, 0).unwrap();
    undo_feedback(&rig.conn, &list, 2).unwrap();
    assert_eq!(playlists::track_ids(&rig.conn, &list).unwrap(), vec![existing]);
    assert_eq!(verdict(&rig.conn, &list, &new).unwrap(), None);
}

#[test]
fn order_and_context_survive_a_restart() {
    let mut rig = Rig::new(vec![
        proposal("A", "One"),
        proposal("B", "Two"),
        proposal("C", "Three"),
    ]);
    let list = playlists::create(&rig.conn, "Set").unwrap();
    rig.discover_for(&list, 3);
    rig.make_ready(|_| 0);
    for t in ["One", "Two"] {
        give_feedback(&rig.conn, &list, &rig.track(t), Verdict::Fits, 1).unwrap();
    }
    playlists::move_entry(&rig.conn, &list, 1, 0).unwrap();
    let order = playlists::track_ids(&rig.conn, &list).unwrap();

    let reopened = crate::db::open(&rig.path).unwrap();
    assert_eq!(playlists::track_ids(&reopened, &list).unwrap(), order);
    assert_eq!(order, vec![rig.track("Two"), rig.track("One")]);
    let three: String = reopened
        .query_row(
            "SELECT id FROM candidate WHERE track_id = ?1",
            params![rig.track("Three")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        contexts(&reopened, &three).unwrap(),
        vec![(list.clone(), "Set".to_string())]
    );
    // A refresh for the playlist does not change accepted tracks or order.
    rig.discover_for(&list, 3);
    assert_eq!(playlists::track_ids(&rig.conn, &list).unwrap(), order);
}

#[test]
fn tracks_that_sound_like_the_playlist_rank_first() {
    let mut rig = Rig::new(vec![
        proposal("A", "Like Members"),
        proposal("B", "Unlike Members"),
        proposal("C", "Member"),
    ]);
    let list = playlists::create(&rig.conn, "Set").unwrap();
    rig.discover_for(&list, 3);
    rig.make_ready(|title| if title == "Unlike Members" { 1 } else { 0 });
    give_feedback(&rig.conn, &list, &rig.track("Member"), Verdict::Fits, 1).unwrap();
    assert_eq!(rig.queue_titles(&list), vec!["Like Members", "Unlike Members"]);
    let top = &queue(&rig.conn, &list, &version(), 1).unwrap()[0];
    assert!(top.reasons.iter().any(|r| r.contains("Sounds like")), "{top:?}");

    // Saying a close match does not fit pushes similar tracks down.
    let other = playlists::create(&rig.conn, "Other").unwrap();
    for t in ["Like Members", "Unlike Members"] {
        let c: String = rig
            .conn
            .query_row(
                "SELECT id FROM candidate WHERE track_id = ?1",
                params![rig.track(t)],
                |r| r.get(0),
            )
            .unwrap();
        add_context(&rig.conn, &c, &other, 0).unwrap();
    }
    give_feedback(&rig.conn, &other, &rig.track("Member"), Verdict::NotForThis, 2).unwrap();
    assert_eq!(rig.queue_titles(&other), vec!["Unlike Members", "Like Members"]);
}

#[test]
fn only_playlists_with_discovery_on_and_a_short_queue_refresh() {
    let mut rig = Rig::new(vec![proposal("A", "One")]);
    let on = playlists::create(&rig.conn, "On").unwrap();
    let off = playlists::create(&rig.conn, "Off").unwrap();
    set_discovery(&rig.conn, &on, true).unwrap();
    let hour = 3_600_000;
    assert_eq!(
        refresh_due(&rig.conn, &version(), 5, hour, 10 * hour).unwrap(),
        vec![on.clone()]
    );
    request_discovery(&rig.conn, "fake_source", &on, 1, 10 * hour).unwrap();
    assert!(
        refresh_due(&rig.conn, &version(), 5, hour, 10 * hour)
            .unwrap()
            .is_empty(),
        "a run is waiting"
    );
    rig.discover_for(&on, 1);
    let _ = off;
    assert!(
        refresh_due(&rig.conn, &version(), 5, hour, 10)
            .unwrap()
            .is_empty(),
        "ran recently"
    );
}
