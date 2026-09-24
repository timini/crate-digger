use std::path::{Path, PathBuf};

use super::*;
use crate::library;
use crate::real_probe::RealProbe;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../audio/tests/fixtures")
        .join(name)
}

/// A library track backed by a copy of `fixture` in `dir`.
fn track(conn: &Connection, dir: &Path, fixture_name: &str, title: &str) -> String {
    let path = dir.join(format!("{title}.{}", fixture_name.rsplit('.').next().unwrap()));
    std::fs::copy(fixture(fixture_name), &path).unwrap();
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "tags",
        &[
            (Field::Artist, Some("Fixture".into())),
            (Field::Title, Some(title.into())),
        ],
    )
    .unwrap();
    let r = library::register_staged(conn, &path, &t, &RealProbe).unwrap();
    conn.execute(
        "UPDATE audio_file SET origin = 'imported' WHERE id = ?1",
        [&r.file_id],
    )
    .unwrap();
    t
}

#[test]
fn a_backup_restores_ratings_playlists_and_asks_to_relink_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    std::fs::create_dir_all(&music).unwrap();
    let a = crate::db::open(&dir.path().join("a.sqlite")).unwrap();
    let t1 = track(&a, &music, "tone.flac", "One");
    let t2 = track(&a, &music, "tone.mp3", "Two");
    review::rate(&a, &t1, RatingKind::Star3, "s", 1).unwrap();
    review::rate(&a, &t2, RatingKind::ThumbsDown, "s", 2).unwrap();
    review::keep(&a, &t1, 3).unwrap();
    a.execute(
        "INSERT INTO seed (id, kind, value, created_at) VALUES ('s1', 'artist', 'Kerri Chandler', 1)",
        [],
    )
    .unwrap();
    let p = playlists::create(&a, "Friday").unwrap();
    playlists::add_tracks(&a, &p, &[t2.clone(), t1.clone()], None).unwrap();

    let snap = snapshot(&a, 10).unwrap();
    assert_eq!(snap.tracks.len(), 2);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(
        !json.contains(&*music.to_string_lossy()),
        "no local paths, only file names"
    );
    let snap: Snapshot = serde_json::from_str(&json).unwrap();

    // A new computer that already has "One" (same file) but not "Two".
    let other_music = dir.path().join("other");
    std::fs::create_dir_all(&other_music).unwrap();
    let b = crate::db::open(&dir.path().join("b.sqlite")).unwrap();
    let local_one = track(&b, &other_music, "tone.flac", "Different Tag Title");
    b.execute(
        "INSERT INTO playlist (id, name, created_at, updated_at) VALUES ('px', 'Friday', 1, 1)",
        [],
    )
    .unwrap();

    let summary = restore(&b, &snap, 20).unwrap();
    assert_eq!(
        (
            summary.matched,
            summary.to_relink,
            summary.ratings,
            summary.playlists,
            summary.seeds
        ),
        (1, 1, 2, 1, 1)
    );
    assert_eq!(
        review::effective_rating(&b, &local_one).unwrap(),
        Some(RatingKind::Star3),
        "matched by file content"
    );
    assert!(review::is_kept(&b, &local_one).unwrap());
    let restored = playlists::list(&b)
        .unwrap()
        .into_iter()
        .find(|p| p.name == "Friday (restored)")
        .unwrap();
    let entries = playlists::entries(&b, &restored.id).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].track_id, local_one, "order kept");

    // The recreated track's file is found again by the relink search.
    std::fs::copy(fixture("tone.mp3"), other_music.join("moved.mp3")).unwrap();
    let proposals = library::find_moved(&b, &other_music, &RealProbe).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].track_id, entries[0].track_id);
    assert_eq!(proposals[0].method, "content");

    // Restoring twice changes nothing more but another playlist copy.
    let again = restore(&b, &snap, 30).unwrap();
    assert_eq!((again.matched, again.ratings), (2, 0));
}
