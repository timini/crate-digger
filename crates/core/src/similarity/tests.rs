use std::sync::atomic::{AtomicBool, AtomicU32};

use super::compute::*;
use super::*;
use crate::analysis::UnitEmbedding;
use crate::domain::{Field, RatingKind};
use crate::{meta, playlists, review};

fn version(model: &str) -> FeatureVersion {
    FeatureVersion {
        model_id: model.into(),
        weights_checksum: "sha256:0".into(),
        preprocessing_version: "p1".into(),
    }
}

/// Deterministic pseudo-random numbers in [-0.5, 0.5).
fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect()
}

fn unit(v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

/// `groups` tight clusters of `per` rows around random centres, then
/// `stray` rows scattered at random.
fn fixture(groups: usize, per: usize, stray: usize, dims: usize) -> Matrix {
    let mut data = vec![];
    for g in 0..groups {
        let centre = noise(1000 + g as u64, dims);
        for i in 0..per {
            let jitter = noise(g as u64 * 100 + i as u64, dims);
            data.extend(unit(
                centre.iter().zip(&jitter).map(|(c, j)| c + 0.05 * j).collect(),
            ));
        }
    }
    for i in 0..stray {
        data.extend(unit(noise(9000 + i as u64, dims)));
    }
    Matrix { dims, data }
}

fn run<T>(f: impl FnOnce(&Control) -> T) -> T {
    let (cancel, progress) = (AtomicBool::new(false), AtomicU32::new(0));
    f(&Control {
        cancel: &cancel,
        progress: &progress,
    })
}

#[test]
fn neighbours_agree_with_a_reference_cosine_calculation() {
    let m = fixture(3, 8, 6, 64);
    let v = version("m");
    let rows: Vec<UnitEmbedding> = (0..m.len())
        .map(|i| Embedding::new(v.clone(), m.row(i).to_vec()).unit())
        .collect();
    let got = run(|c| knn(&m, 5, c)).unwrap();
    for i in 0..m.len() {
        let mut reference: Vec<(usize, f32)> = (0..m.len())
            .filter(|&j| j != i)
            .map(|j| (j, 1.0 - rows[i].cosine(&rows[j]).unwrap()))
            .collect();
        reference.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let ids: Vec<usize> = got[i].iter().map(|x| x.0 as usize).collect();
        let want: Vec<usize> = reference[..5].iter().map(|x| x.0).collect();
        assert_eq!(ids, want, "row {i}");
        for (g, r) in got[i].iter().zip(&reference) {
            assert!((g.1 - r.1).abs() < 1e-4);
        }
    }
}

#[test]
fn dbscan_finds_the_groups_and_leaves_strays_as_noise_reproducibly() {
    let m = fixture(3, 10, 5, 32);
    let first = run(|c| {
        let k = knn(&m, 9, c).unwrap();
        dbscan(&m, &k, 0.05, 5, c).unwrap()
    });
    // Groups are numbered in order of their first row.
    for (g, want) in [(0, 0), (1, 1), (2, 2)] {
        assert!(
            first[g * 10..(g + 1) * 10].iter().all(|l| *l == Some(want)),
            "{first:?}"
        );
    }
    assert!(
        first[30..].iter().all(Option::is_none),
        "strays are noise: {first:?}"
    );
    let second = run(|c| {
        let k = knn(&m, 9, c).unwrap();
        dbscan(&m, &k, 0.05, 5, c).unwrap()
    });
    assert_eq!(first, second);
}

#[test]
fn a_border_track_joins_its_nearest_core_track() {
    // Five close rows make a core; a sixth sits just within eps of one of them.
    let base = unit(noise(1, 16));
    let mut data = vec![];
    for i in 0..5 {
        let j = noise(10 + i, 16);
        data.extend(unit(base.iter().zip(&j).map(|(b, j)| b + 0.01 * j).collect()));
    }
    let far = unit(noise(99, 16));
    let border = unit(base.iter().zip(&far).map(|(b, f)| b + 0.3 * f).collect());
    let d = distance(&border, &data[..16]);
    data.extend(border);
    let m = Matrix { dims: 16, data };
    let labels = run(|c| {
        let k = knn(&m, 5, c).unwrap();
        dbscan(&m, &k, d + 0.01, 5, c).unwrap()
    });
    assert_eq!(labels, vec![Some(0); 6]);
}

#[test]
fn the_layout_is_repeatable_and_keeps_groups_together() {
    let m = fixture(3, 12, 0, 32);
    let a = run(|c| {
        let k = knn(&m, 10, c).unwrap();
        layout(&m, &k, 10, 100, 7, c).unwrap()
    });
    let b = run(|c| {
        let k = knn(&m, 10, c).unwrap();
        layout(&m, &k, 10, 100, 7, c).unwrap()
    });
    assert_eq!(a, b, "the same seed gives the same map");
    let dist = |p: [f32; 2], q: [f32; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
    let (mut within, mut across) = (vec![], vec![]);
    for i in 0..a.len() {
        for j in i + 1..a.len() {
            if i / 12 == j / 12 {
                within.push(dist(a[i], a[j]));
            } else {
                across.push(dist(a[i], a[j]));
            }
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    assert!(
        mean(&within) * 3.0 < mean(&across),
        "{} vs {}",
        mean(&within),
        mean(&across)
    );
}

#[test]
fn a_cancelled_build_stops() {
    let m = fixture(2, 10, 0, 16);
    let (cancel, progress) = (AtomicBool::new(true), AtomicU32::new(0));
    let c = Control {
        cancel: &cancel,
        progress: &progress,
    };
    assert_eq!(knn(&m, 5, &c).unwrap_err(), Cancelled);
}

fn library_track(conn: &Connection, title: &str) -> String {
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "tags",
        &[
            (Field::Artist, Some("Artist".into())),
            (Field::Title, Some(title.into())),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash, availability,
                                 is_primary, last_checked_at, created_at)
         VALUES (?1, ?2, ?3, 'imported', 1, 0, ?1, 'available', 1, 0, 0)",
        params![new_id(), t, format!("/music/{title}.flac")],
    )
    .unwrap();
    t
}

fn embed(conn: &Connection, t: &str, v: &FeatureVersion, values: &[f32], at: i64) {
    let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version, source_fingerprint,
             segment_start_ms, segment_end_ms, dims, embedding, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'fp', 0, 1, ?6, ?7, ?8)",
        params![new_id(), t, v.model_id, v.weights_checksum, v.preprocessing_version, values.len() as i64, bytes, at],
    )
    .unwrap();
}

fn build(conn: &Connection, v: &FeatureVersion, p: &Params) -> MapInfo {
    let inputs = load(conn, v).unwrap();
    let built = run(|c| super::compute(&inputs, p, c)).unwrap();
    save(conn, &inputs, &built, 1_000).unwrap()
}

#[test]
fn the_map_uses_one_model_lists_what_it_cannot_place_and_changes_nothing_else() {
    let conn = crate::db::open_in_memory().unwrap();
    let (v, other) = (version("current"), version("older"));
    let m = fixture(2, 6, 0, 16);
    let mut ids = vec![];
    for i in 0..m.len() {
        let t = library_track(&conn, &format!("Track {i:02}"));
        embed(&conn, &t, &v, m.row(i), 10);
        ids.push(t);
    }
    // Only an older model's embedding, and no embedding at all.
    let old_only = library_track(&conn, "Old model only");
    embed(&conn, &old_only, &other, &[1.0; 16], 10);
    let never = library_track(&conn, "Never analysed");
    // Not in the library: a discovery candidate with an embedding.
    let candidate = meta::create_track(&conn).unwrap();
    embed(&conn, &candidate, &v, m.row(0), 10);

    let list = playlists::create(&conn, "Warm-up").unwrap();
    playlists::add_tracks(&conn, &list, &ids[..3], None).unwrap();
    review::rate(&conn, &ids[0], RatingKind::Star3, "s", 1).unwrap();
    let snapshot = |conn: &Connection| -> (Vec<String>, Option<RatingKind>) {
        (
            playlists::entries(conn, &list)
                .unwrap()
                .into_iter()
                .map(|e| e.track_id)
                .collect(),
            review::effective_rating(conn, &ids[0]).unwrap(),
        )
    };
    let before = snapshot(&conn);

    let map = build(&conn, &v, &Params::default());
    assert_eq!(map.placed, 12);
    assert_eq!(map.version, v);
    assert_eq!(snapshot(&conn), before, "playlists and ratings are untouched");

    let placed: Vec<String> = points(&conn, &map.id)
        .unwrap()
        .into_iter()
        .map(|p| p.track_id)
        .collect();
    assert!(!placed.contains(&old_only) && !placed.contains(&never) && !placed.contains(&candidate));
    let missing: Vec<String> = unplaced(&conn, &v)
        .unwrap()
        .into_iter()
        .map(|u| u.track_id)
        .collect();
    let mut want = vec![old_only.clone(), never.clone()];
    want.sort();
    let mut missing_sorted = missing.clone();
    missing_sorted.sort();
    assert_eq!(missing_sorted, want);
    let cov = coverage(&conn, &v).unwrap();
    assert_eq!((cov.eligible, cov.embedded, cov.stale), (14, 12, None));

    // Playlist membership is reported for colouring, without changing it.
    let p = points(&conn, &map.id).unwrap();
    assert_eq!(p.iter().filter(|p| p.playlists == vec![list.clone()]).count(), 3);

    // Neighbours are stored with their true distance.
    let n = neighbours(&conn, &map.id, &ids[0]).unwrap();
    assert_eq!(n.len(), 10);
    let reference = distance(
        m.row(0),
        m.row(ids.iter().position(|t| *t == n[0].track_id).unwrap()),
    );
    assert!((n[0].distance - reference).abs() < 1e-5);

    // New analysis makes the map stale; a different model says so plainly.
    embed(&conn, &never, &v, m.row(1), 20);
    assert!(coverage(&conn, &v)
        .unwrap()
        .stale
        .unwrap()
        .contains("1 newly analysed"));
    assert!(coverage(&conn, &other)
        .unwrap()
        .stale
        .unwrap()
        .contains("different analysis model"));
}

#[test]
fn provenance_records_the_parameters_actually_used() {
    let conn = crate::db::open_in_memory().unwrap();
    let v = version("current");
    let m = fixture(2, 8, 2, 16);
    for i in 0..m.len() {
        let t = library_track(&conn, &format!("T{i}"));
        embed(&conn, &t, &v, m.row(i), 1);
    }
    let auto = build(&conn, &v, &Params::default());
    assert!(auto.provenance.eps > 0.0);
    assert_eq!(
        auto.provenance.eps_rule,
        "median distance to the 4th nearest neighbour"
    );
    assert_eq!(auto.provenance.pipeline_version, PIPELINE_VERSION);
    let fixed = build(
        &conn,
        &v,
        &Params {
            eps: Some(0.05),
            ..Params::default()
        },
    );
    assert_eq!(
        (fixed.provenance.eps, fixed.provenance.eps_rule.as_str()),
        (0.05, "set by the user")
    );
    assert_eq!(fixed.clusters, 2);
    assert_eq!(fixed.noise, 2);
    // A rebuild replaces the saved map.
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM similarity_map", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn tiny_libraries_still_get_a_map() {
    let conn = crate::db::open_in_memory().unwrap();
    let v = version("current");
    assert_eq!(build(&conn, &v, &Params::default()).placed, 0);
    let t = library_track(&conn, "Only");
    embed(&conn, &t, &v, &[1.0, 0.0, 0.0], 1);
    let map = build(&conn, &v, &Params::default());
    assert_eq!((map.placed, map.clusters, map.noise), (1, 0, 1));
}

/// Timings on 10,000 tracks of 1,280 dimensions, the size of the EffNet
/// embeddings. Run with:
/// cargo test --release -p cd-core similarity::tests::ten_thousand -- --ignored --nocapture
#[test]
#[ignore]
fn ten_thousand_tracks() {
    let m = fixture(40, 240, 400, 1280);
    let inputs = Inputs {
        version: version("bench"),
        ids: (0..m.len()).map(|i| format!("{i:05}")).collect(),
        matrix: m,
        newest_feature_at: 0,
    };
    let started = Instant::now();
    let built = run(|c| super::compute(&inputs, &Params::default(), c)).unwrap();
    let clusters = built.points.iter().filter_map(|p| p.1).max().map_or(0, |c| c + 1);
    println!(
        "10k map: {:?} total, {} clusters, {} noise",
        started.elapsed(),
        clusters,
        built.points.iter().filter(|p| p.1.is_none()).count()
    );
}
