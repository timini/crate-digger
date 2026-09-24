use super::*;
use crate::domain::{Field, RatingKind};
use crate::util::new_id;
use crate::{meta, review};

fn version() -> FeatureVersion {
    FeatureVersion {
        model_id: "m".into(),
        weights_checksum: "sha256:0".into(),
        preprocessing_version: "p1".into(),
    }
}

/// A discovered track of `style` (0 or 1), with the same cultural evidence
/// for every track so that evidence alone cannot tell the styles apart.
fn discovered(conn: &Connection, style: usize, i: usize) -> String {
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "tags",
        &[
            (Field::Artist, Some(format!("Private Artist {i}"))),
            (Field::Title, Some(format!("Private Title {i}"))),
        ],
    )
    .unwrap();
    let c = new_id();
    conn.execute(
        "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES (?1, ?2, 'reviewed', 0, 0)",
        params![c, t],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO evidence (id, candidate_id, source_kind, source_url, retrieved_at, excerpt, confidence)
         VALUES (?1, ?2, 'discogs', 'https://example.test', 0, 'x', 0.5)",
        params![new_id(), c],
    )
    .unwrap();
    let mut v = [0f32; 16];
    v[style] = 1.0;
    v[2 + i % 14] += 0.2;
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    let fv = version();
    conn.execute(
        "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version, source_fingerprint,
             segment_start_ms, segment_end_ms, dims, embedding, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'fp', 0, 1, 16, ?6, 0)",
        params![new_id(), t, fv.model_id, fv.weights_checksum, fv.preprocessing_version, bytes],
    )
    .unwrap();
    t
}

#[test]
fn auc_counts_ties_as_half() {
    assert_eq!(auc(&[(0.9, true), (0.1, false)]), Some(1.0));
    assert_eq!(auc(&[(0.1, true), (0.9, false)]), Some(0.0));
    assert_eq!(auc(&[(0.5, true), (0.5, false)]), Some(0.5));
    assert_eq!(auc(&[(0.5, true)]), None);
}

#[test]
fn replay_uses_only_earlier_ratings_and_shows_what_taste_adds() {
    let conn = crate::db::open_in_memory().unwrap();
    let mut tracks = vec![];
    for i in 0..40 {
        let style = i % 2;
        let t = discovered(&conn, style, i);
        let kind = if style == 0 {
            RatingKind::Star3
        } else {
            RatingKind::ThumbsDown
        };
        review::rate(&conn, &t, kind, "s", i as i64).unwrap();
        tracks.push(t);
    }
    // A later change of mind is not a first judgement.
    review::rate(&conn, &tracks[0], RatingKind::ThumbsDown, "s", 100).unwrap();
    review::flag_wrong_version(&conn, &tracks[1], true, 101).unwrap();

    let r = report(&conn, &version(), &ranking::DEFAULT, 10, "0.1.0", 200).unwrap();
    assert_eq!(r.ranking.judgements, 40);
    assert_eq!(r.ranking.strong_positives, 20);
    assert_eq!(r.outcomes.three_stars, 20);
    assert_eq!(r.outcomes.thumbs_down, 20);
    assert_eq!(r.outcomes.wrong_version_rate, Some(1.0 / 40.0));
    // Evidence is the same for every track, so alone it cannot rank.
    assert_eq!(r.ranking.auc_cultural, Some(0.5));
    // Early judgements have little history, so the combined score is not
    // perfect, but it is clearly better.
    let combined = r.ranking.auc_combined.unwrap();
    assert!(combined > 0.8, "{combined}");
    let (lo, hi) = r.ranking.auc_difference_interval.unwrap();
    assert!(lo > 0.0 && hi >= lo, "{lo} {hi}");

    // The first judgement cannot see itself or anything after it.
    let first = first_judgements(&conn).unwrap();
    assert!(ratings_before(&conn, first[0].rowid).unwrap().is_empty());
    let before_last = ratings_before(&conn, first[39].rowid).unwrap();
    assert_eq!(before_last.len(), 39);
    assert!(before_last.iter().all(|(t, _)| *t != first[39].track));
}

#[test]
fn undone_judgements_are_replaced_by_the_next_one() {
    let conn = crate::db::open_in_memory().unwrap();
    let t = discovered(&conn, 0, 0);
    review::rate(&conn, &t, RatingKind::ThumbsDown, "s", 1).unwrap();
    review::undo(&conn, "s", 2).unwrap();
    review::rate(&conn, &t, RatingKind::Star2, "s", 3).unwrap();
    let first = first_judgements(&conn).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, "star2");
}

#[test]
fn the_report_holds_no_names_or_ids() {
    let conn = crate::db::open_in_memory().unwrap();
    let t = discovered(&conn, 0, 7);
    review::rate(&conn, &t, RatingKind::Star1, "s", 1).unwrap();
    let r = report(&conn, &version(), &ranking::DEFAULT, 10, "0.1.0", 2).unwrap();
    let json = serde_json::to_string(&r).unwrap();
    assert!(!json.contains(&t));
    assert!(!json.contains("Private"));
    assert!(!json.contains("example.test"));
}
