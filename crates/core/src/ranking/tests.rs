use rusqlite::{params, Connection};

use super::*;

fn version(model: &str) -> FeatureVersion {
    FeatureVersion {
        model_id: model.into(),
        weights_checksum: "sha256:0".into(),
        preprocessing_version: "p1".into(),
    }
}

/// A deterministic vector near axis `axis` of `dims`, with small noise.
fn near(axis: usize, noise_seed: u64, v: &FeatureVersion) -> Embedding {
    let dims = 16;
    let mut x = noise_seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let vector = (0..dims)
        .map(|i| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let noise = ((x >> 33) as f32 / u32::MAX as f32 - 0.25) * 0.2;
            if i == axis {
                1.0 + noise
            } else {
                noise
            }
        })
        .collect();
    Embedding::new(v.clone(), vector)
}

fn blend(a: &Embedding, b: &Embedding) -> Embedding {
    let va = a.to_bytes();
    let vb = b.to_bytes();
    let fa: Vec<f32> = va
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let fb: Vec<f32> = vb
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    Embedding::new(
        a.version().clone(),
        fa.iter().zip(&fb).map(|(x, y)| (x + y) / 2.0).collect(),
    )
}

fn positives(axis: usize, n: usize, v: &FeatureVersion) -> Vec<Positive> {
    (0..n)
        .map(|i| Positive {
            embedding: near(axis, 100 * axis as u64 + i as u64, v).unit(),
            weight: 1.0,
        })
        .collect()
}

fn item(id: &str, e: Option<Embedding>, evidence: f32, artist: &str) -> Item {
    Item {
        id: id.into(),
        embedding: e.map(|e| e.unit()),
        evidence,
        seed_match: false,
        artist: artist.into(),
    }
}

#[test]
fn separate_tastes_stay_separate() {
    let v = version("m");
    let mut p = positives(0, 5, &v);
    p.extend(positives(1, 5, &v));
    let profile = Profile::build(p, vec![], &DEFAULT);
    assert_eq!(profile.clusters.len(), 2);
    let a = score(&profile, &item("a", Some(near(0, 999, &v)), 0.5, "x"), &DEFAULT);
    let b = score(&profile, &item("b", Some(near(1, 998, &v)), 0.5, "y"), &DEFAULT);
    let between = score(
        &profile,
        &item("m", Some(blend(&near(0, 997, &v), &near(1, 996, &v))), 0.5, "z"),
        &DEFAULT,
    );
    let elsewhere = score(&profile, &item("e", Some(near(5, 995, &v)), 0.5, "w"), &DEFAULT);
    assert_ne!(a.cluster, b.cluster);
    // A single average of both tastes would favour the blend; clusters do not.
    assert!(
        a.score > between.score && b.score > between.score,
        "{a:?} {b:?} {between:?}"
    );
    assert!(between.score > elsewhere.score);
}

#[test]
fn dislikes_suppress_similar_sounds_not_artists() {
    let v = version("m");
    let disliked = near(2, 1, &v);
    let profile = Profile::build(positives(0, 6, &v), vec![disliked.unit()], &DEFAULT);
    let same_sound = score(
        &profile,
        &item("s", Some(near(2, 2, &v)), 0.8, "other artist"),
        &DEFAULT,
    );
    // Same artist as the disliked track, different sound: no penalty.
    let same_artist = score(
        &profile,
        &item("a", Some(near(3, 3, &v)), 0.8, "disliked artist"),
        &DEFAULT,
    );
    assert!(same_sound.dislike > 0.5, "{same_sound:?}");
    assert_eq!(same_artist.dislike, 0.0);
    assert!(same_artist.score > same_sound.score);
}

#[test]
fn cold_start_ranks_by_evidence_and_seeds() {
    let v = version("m");
    let profile = Profile::build(positives(0, 2, &v), vec![], &DEFAULT);
    let tasteful = score(&profile, &item("t", Some(near(0, 50, &v)), 0.2, "a"), &DEFAULT);
    let supported = score(&profile, &item("s", Some(near(4, 51, &v)), 0.9, "b"), &DEFAULT);
    assert!(supported.score > tasteful.score);
    let mut seeded = item("d", None, 0.5, "c");
    seeded.seed_match = true;
    assert!(
        score(&profile, &seeded, &DEFAULT).score
            > score(&profile, &item("u", None, 0.5, "e"), &DEFAULT).score
    );

    // With enough positives taste leads.
    let warm = Profile::build(positives(0, 6, &v), vec![], &DEFAULT);
    let tasteful = score(&warm, &item("t", Some(near(0, 50, &v)), 0.2, "a"), &DEFAULT);
    let supported = score(&warm, &item("s", Some(near(4, 51, &v)), 0.9, "b"), &DEFAULT);
    assert!(tasteful.score > supported.score);
}

#[test]
fn every_fifth_position_is_exploration_when_available() {
    let v = version("m");
    let profile = Profile::build(positives(0, 6, &v), vec![], &DEFAULT);
    let mut scored = vec![];
    for i in 0..40 {
        scored.push(score(
            &profile,
            &item(
                &format!("p{i}"),
                Some(near(0, 200 + i, &v)),
                0.3,
                &format!("a{i}"),
            ),
            &DEFAULT,
        ));
    }
    for i in 0..20 {
        scored.push(score(
            &profile,
            &item(
                &format!("x{i}"),
                Some(near(7, 300 + i, &v)),
                0.8,
                &format!("b{i}"),
            ),
            &DEFAULT,
        ));
    }
    let ordered = order(scored, &profile, &DEFAULT);
    assert_eq!(ordered.len(), 60);
    for (i, (s, slot)) in ordered.iter().enumerate().take(50) {
        let expect = if (i + 1) % 5 == 0 {
            Slot::Exploration
        } else {
            Slot::Personal
        };
        assert_eq!(*slot, expect, "position {} {}", i + 1, s.id);
    }
    let first_fifty = ordered
        .iter()
        .take(50)
        .filter(|(_, s)| *s == Slot::Exploration)
        .count();
    assert_eq!(first_fifty, 10, "20% of positions");
    // Leftover exploration candidates fill the end rather than disappearing.
    assert_eq!(
        ordered.iter().filter(|(_, s)| *s == Slot::Exploration).count(),
        20
    );
}

#[test]
fn no_exploration_slots_during_cold_start() {
    let v = version("m");
    let profile = Profile::build(positives(0, 2, &v), vec![], &DEFAULT);
    let scored: Vec<Scored> = (0..10)
        .map(|i| {
            score(
                &profile,
                &item(&format!("x{i}"), Some(near(7, i, &v)), 0.9, "a"),
                &DEFAULT,
            )
        })
        .collect();
    assert!(order(scored, &profile, &DEFAULT)
        .iter()
        .all(|(_, s)| *s == Slot::Personal));
}

#[test]
fn the_same_artist_is_not_queued_back_to_back_when_avoidable() {
    let v = version("m");
    let profile = Profile::build(positives(0, 6, &v), vec![], &DEFAULT);
    let mut scored = vec![];
    for i in 0..2 {
        scored.push(score(
            &profile,
            &item(&format!("s{i}"), Some(near(0, 400 + i, &v)), 0.5, "same"),
            &DEFAULT,
        ));
    }
    for i in 0..3 {
        scored.push(score(
            &profile,
            &item(
                &format!("o{i}"),
                Some(near(0, 500 + i, &v)),
                0.45,
                &format!("other{i}"),
            ),
            &DEFAULT,
        ));
    }
    let ordered = order(scored, &profile, &DEFAULT);
    for w in ordered.windows(2) {
        assert!(
            !(w[0].0.artist == "same" && w[1].0.artist == "same"),
            "{:?}",
            ordered.iter().map(|(s, _)| &s.id).collect::<Vec<_>>()
        );
    }
}

#[test]
fn other_versions_never_mix() {
    let a = version("a");
    let b = version("b");
    let profile = Profile::build(positives(0, 6, &a), vec![near(0, 1, &a).unit()], &DEFAULT);
    let s = score(&profile, &item("x", Some(near(0, 9, &b)), 0.5, "z"), &DEFAULT);
    assert_eq!((s.taste, s.dislike, s.cluster), (0.0, 0.0, None));
}

struct Db {
    conn: Connection,
    _dir: tempfile::TempDir,
    v: FeatureVersion,
}

impl Db {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
        Db {
            conn,
            _dir: dir,
            v: version("m"),
        }
    }

    /// A track with an embedding near `axis`, as a candidate in `stage`.
    fn track(&self, artist: &str, axis: usize, seed: u64, stage: Option<&str>, evidence: f64) -> String {
        let track = crate::meta::create_track(&self.conn).unwrap();
        crate::meta::set_extracted(
            &self.conn,
            &track,
            "t",
            &[(crate::domain::Field::Artist, Some(artist.into()))],
        )
        .unwrap();
        self.conn
            .execute(
                "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version,
                     source_fingerprint, segment_start_ms, segment_end_ms, dims, embedding, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'fp', 0, 1, 16, ?6, 1)",
                params![
                    crate::util::new_id(),
                    track,
                    self.v.model_id,
                    self.v.weights_checksum,
                    self.v.preprocessing_version,
                    near(axis, seed, &self.v).to_bytes()
                ],
            )
            .unwrap();
        if let Some(stage) = stage {
            let id = crate::util::new_id();
            self.conn
                .execute(
                    "INSERT INTO candidate (id, track_id, stage, verified, confidence, score, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 1, ?4, ?4, 1, 1)",
                    params![id, track, stage, evidence],
                )
                .unwrap();
            self.conn
                .execute(
                    "INSERT INTO evidence (id, candidate_id, source_kind, source_url, retrieved_at, excerpt, confidence)
                     VALUES (?1, ?2, 'page', 'https://example.invalid', 1, 'x', ?3)",
                    params![crate::util::new_id(), id, evidence],
                )
                .unwrap();
        }
        track
    }

    fn queue(&self) -> Vec<String> {
        self.conn
            .prepare(
                "SELECT m.artist FROM candidate c JOIN track_meta m USING (track_id)
                 WHERE c.queue_rank IS NOT NULL ORDER BY c.queue_rank",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }
}

#[test]
fn ratings_reorder_the_queue_and_undo_restores_it() {
    let db = Db::new();
    let ready_a = db.track("sounds like a", 0, 1, Some("ready"), 0.5);
    let _ready_b = db.track("sounds like b", 1, 2, Some("ready"), 0.6);
    db.track("not ready yet", 0, 3, Some("downloading"), 0.9);
    let liked: Vec<String> = (0..5)
        .map(|i| db.track(&format!("liked {i}"), 0, 10 + i, None, 0.0))
        .collect();

    // Cold start: evidence decides.
    let s = crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    assert_eq!((s.candidates, s.queued, s.positives), (3, 2, 0));
    assert_eq!(db.queue(), vec!["sounds like b", "sounds like a"]);

    let scores = |db: &Db| -> Vec<f64> {
        db.conn
            .prepare("SELECT score FROM candidate ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    for t in &liked[..4] {
        crate::review::rate(&db.conn, t, crate::domain::RatingKind::Star3, "s", 2).unwrap();
    }
    crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    let before = scores(&db);
    crate::review::rate(&db.conn, &liked[4], crate::domain::RatingKind::Star3, "s", 2).unwrap();
    let s = crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    assert_eq!((s.positives, s.clusters), (5, 1));
    assert_eq!(db.queue(), vec!["sounds like a", "sounds like b"]);
    assert_ne!(scores(&db), before);

    // Undo restores the previous preference and its ranking.
    crate::review::undo(&db.conn, "s", 3).unwrap();
    crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    assert_eq!(scores(&db), before);

    // A thumbs down on the candidate's sound lowers it.
    let _ = ready_a;
    crate::review::rate(&db.conn, &liked[4], crate::domain::RatingKind::Star3, "s", 4).unwrap();
    let hated = db.track("disliked", 0, 20, None, 0.0);
    crate::review::rate(&db.conn, &hated, crate::domain::RatingKind::ThumbsDown, "s", 5).unwrap();
    let score_a = |db: &Db| -> f64 {
        db.conn
            .query_row(
                "SELECT c.score FROM candidate c JOIN track_meta m USING (track_id) WHERE m.artist = 'sounds like a'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    let with_dislike = score_a(&db);
    crate::review::undo(&db.conn, "s", 6).unwrap();
    crate::ranking::rerank(&db.conn, &db.v, &DEFAULT).unwrap();
    assert!(score_a(&db) > with_dislike);
}

#[test]
fn embeddings_of_another_version_are_ignored() {
    let db = Db::new();
    db.track("candidate", 0, 1, Some("ready"), 0.5);
    for i in 0..5 {
        let t = db.track(&format!("liked {i}"), 0, 10 + i, None, 0.0);
        crate::review::rate(&db.conn, &t, crate::domain::RatingKind::Star3, "s", 2).unwrap();
    }
    let s = crate::ranking::rerank(&db.conn, &version("another model"), &DEFAULT).unwrap();
    assert_eq!(s.positives, 0);
}

/// Run with --release: `cargo test --release -p cd-core --lib rerank_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing"]
fn rerank_timing_for_a_thousand_candidates() {
    let db = Db::new();
    let v = db.v.clone();
    let big = |axis: usize, seed: u64| -> Vec<u8> {
        let mut x = seed;
        let vector: Vec<f32> = (0..1280)
            .map(|i| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((x >> 40) as f32 / (1u64 << 24) as f32 - 0.5) + if i % 6 == axis { 0.5 } else { 0.0 }
            })
            .collect();
        Embedding::new(v.clone(), vector).to_bytes()
    };
    let tx = db.conn.unchecked_transaction().unwrap();
    for i in 0..1300u64 {
        let track = crate::meta::create_track(&tx).unwrap();
        tx.execute(
            "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version,
                 source_fingerprint, segment_start_ms, segment_end_ms, dims, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'fp', 0, 1, 1280, ?6, 1)",
            params![
                crate::util::new_id(),
                track,
                v.model_id,
                v.weights_checksum,
                v.preprocessing_version,
                big((i % 6) as usize, i)
            ],
        )
        .unwrap();
        if i < 300 {
            let kind = if i % 4 == 0 { "thumbs_down" } else { "star2" };
            tx.execute(
                "INSERT INTO rating_event (id, track_id, kind, session_id, created_at) VALUES (?1, ?2, ?3, 's', 1)",
                params![crate::util::new_id(), track, kind],
            )
            .unwrap();
        } else {
            tx.execute(
                "INSERT INTO candidate (id, track_id, stage, verified, confidence, score, created_at, updated_at)
                 VALUES (?1, ?2, 'ready', 1, 0.5, 0.5, 1, 1)",
                params![crate::util::new_id(), track],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
    let started = std::time::Instant::now();
    let s = rerank(&db.conn, &v, &DEFAULT).unwrap();
    let elapsed = started.elapsed();
    println!(
        "rerank of {} candidates with {} positives: {:.3} s",
        s.candidates,
        s.positives,
        elapsed.as_secs_f64()
    );
    assert_eq!(s.candidates, 1000);
}
