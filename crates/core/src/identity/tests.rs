use rusqlite::{params, Connection};

use super::policy::{decide, Evidence, Relation, Thresholds, Verdict};
use super::*;
use crate::domain::Field;

fn track(conn: &Connection, artist: &str, title: &str, mix: Option<&str>, secs: i64, path: &str) -> String {
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "tags",
        &[
            (Field::Artist, Some(artist.into())),
            (Field::Title, Some(title.into())),
            (Field::Mix, mix.map(str::to_string)),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash, duration_ms,
                                 is_primary, last_checked_at, created_at)
         VALUES (?1, ?2, ?3, 'imported', 1, 0, ?1, ?4, 1, 0, 0)",
        params![crate::util::new_id(), t, path, secs * 1000],
    )
    .unwrap();
    t
}

fn fp(score: f64, coverage: f64, speed: f64) -> Evidence {
    Evidence::FingerprintMatch {
        score,
        coverage,
        speed,
    }
}

fn run(conn: &Connection, a: &str, b: &str, evidence: &[Evidence]) -> Applied {
    let d = decide(
        &side_for_track(conn, a).unwrap(),
        &side_for_track(conn, b).unwrap(),
        evidence,
        &Thresholds::default(),
    );
    apply(conn, a, b, &d, evidence, "test").unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn fingerprint_match_merges_with_recorded_evidence() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", None, 300, "/m/a.flac");
    let b = track(&conn, "A", "T", Some("Original Mix"), 300, "/m/a.mp3");
    assert_eq!(
        run(&conn, &a, &b, &[fp(0.9, 0.99, 1.0)]),
        Applied::Merged { kept: a.clone() }
    );
    assert_eq!(library::files_for_track(&conn, &a).unwrap().len(), 2);
    let evidence: String = conn
        .query_row(
            "SELECT evidence FROM track_redirect WHERE old_id = ?1",
            [&b],
            |r| r.get(0),
        )
        .unwrap();
    assert!(evidence.contains("fingerprints match"), "{evidence}");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM identity_evidence WHERE verdict = 'same_recording'"
        ),
        1
    );
}

#[test]
fn names_alone_change_nothing() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", None, 300, "/m/a.flac");
    let b = track(&conn, "A", "T", None, 300, "/m/b.flac");
    assert_eq!(run(&conn, &a, &b, &[]), Applied::NoChange);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM track"), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM identity_conflict"), 0);
}

#[test]
fn different_mixes_are_linked_as_versions_of_one_work() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", Some("Original Mix"), 300, "/m/a.flac");
    let b = track(&conn, "A", "T", Some("Dub"), 330, "/m/b.flac");
    let c = track(&conn, "A", "T", Some("Radio Edit"), 200, "/m/c.flac");
    assert!(matches!(
        run(&conn, &a, &b, &[]),
        Applied::LinkedAsVersions { .. }
    ));
    assert!(matches!(
        run(&conn, &c, &a, &[]),
        Applied::LinkedAsVersions { .. }
    ));
    let v = versions(&conn, &a).unwrap();
    assert_eq!(v.len(), 2);
    assert!(v.iter().all(|x| x.has_audio));
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM track"),
        3,
        "versions are never merged"
    );
    assert_eq!(count(&conn, "SELECT COUNT(DISTINCT work_id) FROM track"), 1);
}

#[test]
fn linking_combines_existing_works() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", Some("Dub"), 300, "/m/a.flac");
    let b = track(&conn, "A", "T", Some("Radio Edit"), 300, "/m/b.flac");
    let c = track(&conn, "A", "T", Some("X Remix"), 300, "/m/c.flac");
    let d = track(&conn, "A", "T", Some("Y Remix"), 300, "/m/d.flac");
    link_versions(&conn, &a, &b).unwrap();
    link_versions(&conn, &c, &d).unwrap();
    link_versions(&conn, &b, &c).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(DISTINCT work_id) FROM track"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM work"), 1);
}

#[test]
fn conflicts_go_to_review_and_the_user_resolves_them() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", Some("Extended Mix"), 300, "/m/a.flac");
    let b = track(&conn, "A", "T", Some("Original Mix"), 300, "/m/b.flac");
    assert_eq!(run(&conn, &a, &b, &[fp(0.9, 0.99, 1.0)]), Applied::SentToReview);
    // Running the same comparison again does not duplicate the conflict.
    run(&conn, &a, &b, &[fp(0.9, 0.99, 1.0)]);
    let open = open_conflicts(&conn).unwrap();
    assert_eq!(open.len(), 1);
    assert!(open[0].reason.contains("different mixes"));
    assert_eq!(
        open[0].a.path.as_deref().map(|p| p.starts_with("/m/")),
        Some(true)
    );

    let applied = resolve_conflict(&conn, &open[0].id, Relation::SameRecording).unwrap();
    assert!(matches!(applied, Applied::Merged { .. }));
    assert!(open_conflicts(&conn).unwrap().is_empty());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM track"), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM identity_evidence WHERE source = 'user'"
        ),
        1
    );
}

#[test]
fn resolving_as_unrelated_keeps_both_and_stops_duplicate_suggestions() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", None, 300, "/m/a.flac");
    let b = track(&conn, "A", "T", None, 200, "/m/b.flac");
    run(&conn, &a, &b, &[Evidence::FingerprintMismatch { score: 0.05 }]);
    let c = open_conflicts(&conn).unwrap().remove(0);
    resolve_conflict(&conn, &c.id, Relation::Unrelated).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM track"), 2);
    assert!(library::duplicates::suggestions(&conn, 10).unwrap().is_empty());
    assert!(
        resolve_conflict(&conn, &c.id, Relation::SameRecording).is_err(),
        "already resolved"
    );
}

#[test]
fn pitched_copy_joins_the_track_as_a_variant_and_is_never_primary() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", None, 400, "/m/a.flac");
    let b = track(&conn, "A", "T", None, 385, "/m/a-pitched.mp3");
    // B plays 4% faster than A; B is the pitched copy.
    let r = run(&conn, &a, &b, &[fp(0.8, 0.95, 1.04)]);
    assert_eq!(
        r,
        Applied::MergedAsPitchedCopy {
            kept: a.clone(),
            percent: 4.0
        }
    );
    let files = library::files_for_track(&conn, &a).unwrap();
    assert_eq!(files.len(), 2);
    let primary = files.iter().find(|f| f.is_primary).unwrap();
    let variant: Option<String> = conn
        .query_row(
            "SELECT variant FROM audio_file WHERE id = ?1",
            [&primary.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(variant, None, "the unpitched copy plays");
    assert_eq!(primary.path, "/m/a.flac");
    let (path, pitched): (String, String) = conn
        .query_row(
            "SELECT path, variant FROM audio_file WHERE variant IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (path.as_str(), pitched.as_str()),
        ("/m/a-pitched.mp3", "pitched:+4.0")
    );
}

#[test]
fn side_uses_unpitched_duration_and_recording_ids() {
    let conn = crate::db::open_in_memory().unwrap();
    let a = track(&conn, "A", "T", None, 400, "/m/a.flac");
    conn.execute(
        "INSERT INTO track_external_id (track_id, namespace, value, source) VALUES (?1, 'isrc', 'X1', 'tags')",
        [&a],
    )
    .unwrap();
    let s = side_for_track(&conn, &a).unwrap();
    assert_eq!(s.duration_ms, Some(400_000));
    assert_eq!(s.external_ids, vec![("isrc".to_string(), "X1".to_string())]);
    let v = decide(&s, &s.clone(), &[], &Thresholds::default()).verdict;
    assert_eq!(v, Verdict::SameRecording);
}
