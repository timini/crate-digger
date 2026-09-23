use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};

use super::search::{search, LibraryQuery, RatingFilter};
use super::*;
use crate::domain::Field;

use crate::real_probe::RealProbe;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures")
}

fn copy_fixture(name: &str, dest_dir: &Path, as_name: &str) -> PathBuf {
    std::fs::create_dir_all(dest_dir).unwrap();
    let dest = dest_dir.join(as_name);
    std::fs::copy(fixtures().join(name), &dest).unwrap();
    dest
}

/// A small library: nested folders, a hidden file, a non-audio file and
/// corrupt audio.
fn build_library(dir: &Path) {
    copy_fixture("tone.flac", &dir.join("House"), "01 Night Signal.flac");
    copy_fixture("tone.mp3", &dir.join("House/Promos"), "night-signal.mp3");
    copy_fixture("other-stereo.flac", &dir.join("Techno"), "Four Forty.flac");
    copy_fixture("unicode.flac", dir, "Überlicht.flac");
    copy_fixture("tone.wav", &dir.join("Samples"), "tone.wav");
    copy_fixture("corrupt.mp3", dir, "broken.mp3");
    copy_fixture("tone.flac", &dir.join(".hidden"), "secret.flac");
    copy_fixture("tone.flac", dir, "._resource-fork.flac");
    std::fs::write(dir.join("cover.jpg"), b"not audio").unwrap();
}

type Snapshot = BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>, String)>;

fn snapshot(dir: &Path) -> Snapshot {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .map(|e| e.unwrap())
        .map(|e| {
            let m = e.metadata().unwrap();
            if m.is_file() {
                let bytes = blake3::hash(&std::fs::read(e.path()).unwrap())
                    .to_hex()
                    .to_string();
                (
                    e.path().to_path_buf(),
                    (m.len(), Some(m.modified().unwrap()), bytes),
                )
            } else {
                // Folder timestamps are left out: Windows updates them lazily
                // after files are written, independent of anything we do.
                (e.path().to_path_buf(), (0, None, "dir".into()))
            }
        })
        .collect()
}

fn import(conn: &Connection, dir: &Path) -> ImportSummary {
    let root = add_root(conn, dir).unwrap();
    import_root(conn, &root, &RealProbe, |_| Ok::<(), ()>(())).unwrap()
}

fn track_by_title(conn: &Connection, title: &str) -> String {
    conn.query_row(
        "SELECT track_id FROM track_meta WHERE title = ?1",
        params![title],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn import_never_renames_moves_or_modifies_files() {
    let dir = tempfile::tempdir().unwrap();
    build_library(dir.path());
    let before = snapshot(dir.path());

    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    import(&conn, dir.path()); // and rescanning

    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn import_indexes_supported_files_and_skips_hidden_ones() {
    let dir = tempfile::tempdir().unwrap();
    build_library(dir.path());
    let conn = crate::db::open_in_memory().unwrap();
    let s = import(&conn, dir.path());

    assert_eq!(s.total, 6, "{s:?}");
    assert_eq!(s.added, 6);
    assert_eq!(s.duplicate_copies, 0);
    assert_eq!(s.corrupt, 1);
    assert!(s.errors.is_empty(), "{:?}", s.errors);
    let hidden: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_file WHERE instr(path, 'secret') > 0 OR instr(path, '._') > 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hidden, 0);
}

#[test]
fn tags_are_read_with_mix_split_from_title() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("tone.flac", dir.path(), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let t = track_by_title(&conn, "Night Signal");
    let m = meta::effective(&conn, &t).unwrap();
    assert_eq!(m.artist.as_deref(), Some("Fixture Collective"));
    assert_eq!(m.mix.as_deref(), Some("Extended Mix"));
    assert_eq!(m.release.as_deref(), Some("Test Pressings EP"));
    assert_eq!(m.label.as_deref(), Some("Test Pressings"));
    assert_eq!(m.track_number.as_deref(), Some("3"));
    assert_eq!(m.year, Some(2024));
    assert_eq!(m.tempo, Some(124.0));
    assert_eq!(m.musical_key.as_deref(), Some("8A"));

    let f = playable_file(&conn, &t).unwrap().unwrap();
    assert_eq!(f.format.as_deref(), Some("flac"));
    assert!((1900..=2100).contains(&f.duration_ms.unwrap()));
}

#[test]
fn mp3_id3_tags_are_read() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("tone.mp3", dir.path(), "a.mp3");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let t = track_by_title(&conn, "Night Signal");
    let m = meta::effective(&conn, &t).unwrap();
    assert_eq!(m.label.as_deref(), Some("Test Pressings"));
    assert_eq!(m.tempo, Some(124.0));
    assert_eq!(m.musical_key.as_deref(), Some("8A"));
}

#[test]
fn corrupt_file_keeps_its_record_with_an_actionable_reason() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("corrupt.mp3", dir.path(), "broken.mp3");
    copy_fixture("truncated.flac", dir.path(), "cut.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let rows: Vec<(String, Option<String>)> = conn
        .prepare("SELECT availability, availability_reason FROM audio_file")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (availability, reason) in rows {
        assert_eq!(availability, "corrupt");
        assert!(reason.unwrap().contains("replace it with a good copy"));
    }
}

#[test]
fn missing_file_keeps_metadata_rating_and_playlist() {
    let dir = tempfile::tempdir().unwrap();
    let file = copy_fixture("tone.flac", dir.path(), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let t = track_by_title(&conn, "Night Signal");
    rate(&conn, &t, "star3");
    add_to_playlist(&conn, &t);

    std::fs::remove_file(&file).unwrap();
    let s = import(&conn, dir.path());
    assert_eq!(s.marked_missing, 1);

    let f = &files_for_track(&conn, &t).unwrap()[0];
    assert_eq!(f.availability, Availability::Missing);
    assert!(f.availability_reason.as_ref().unwrap().contains("Use Relink"));
    assert!(playable_file(&conn, &t).unwrap().is_none());
    assert_eq!(
        meta::effective(&conn, &t).unwrap().artist.as_deref(),
        Some("Fixture Collective")
    );
    assert_eq!(rating_of(&conn, &t).as_deref(), Some("star3"));
    assert_eq!(playlist_entries(&conn, &t), 1);
}

fn rate(conn: &Connection, track: &str, kind: &str) {
    conn.execute(
        "INSERT INTO rating_event (id, track_id, kind, session_id, created_at) VALUES (?1, ?2, ?3, 's', 1)",
        params![crate::util::new_id(), track, kind],
    )
    .unwrap();
}

fn rating_of(conn: &Connection, track: &str) -> Option<String> {
    conn.query_row(
        "SELECT kind FROM effective_rating WHERE track_id = ?1",
        params![track],
        |r| r.get(0),
    )
    .ok()
}

fn add_to_playlist(conn: &Connection, track: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO playlist (id, name, created_at, updated_at) VALUES ('p1', 'Set', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO playlist_entry (playlist_id, position, track_id, added_at)
         VALUES ('p1', (SELECT COUNT(*) FROM playlist_entry WHERE playlist_id = 'p1'), ?1, 0)",
        params![track],
    )
    .unwrap();
}

fn playlist_entries(conn: &Connection, track: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM playlist_entry WHERE track_id = ?1",
        params![track],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn relinking_a_moved_file_restores_playback_ratings_and_playlists() {
    let dir = tempfile::tempdir().unwrap();
    let old = copy_fixture("tone.flac", &dir.path().join("old"), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, &dir.path().join("old"));
    let t = track_by_title(&conn, "Night Signal");
    rate(&conn, &t, "star2");
    add_to_playlist(&conn, &t);

    let new_dir = dir.path().join("new/place");
    std::fs::create_dir_all(&new_dir).unwrap();
    let new = new_dir.join("renamed.flac");
    std::fs::rename(&old, &new).unwrap();
    import(&conn, &dir.path().join("old"));
    assert!(playable_file(&conn, &t).unwrap().is_none());

    let proposals = find_moved(&conn, &dir.path().join("new"), &RealProbe).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].method, "content");
    assert_eq!(proposals[0].track_id, t);
    relink_file(
        &conn,
        &proposals[0].file_id,
        Path::new(&proposals[0].new_path),
        &RealProbe,
    )
    .unwrap();

    let f = playable_file(&conn, &t).unwrap().unwrap();
    assert_eq!(Path::new(&f.path), new);
    assert!(cd_audio::probe(Path::new(&f.path)).is_ok());
    assert_eq!(rating_of(&conn, &t).as_deref(), Some("star2"));
    assert_eq!(playlist_entries(&conn, &t), 1);
}

#[test]
fn importing_a_folder_containing_a_moved_file_relinks_it_automatically() {
    let dir = tempfile::tempdir().unwrap();
    let old = copy_fixture("tone.flac", &dir.path().join("old"), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, &dir.path().join("old"));
    let t = track_by_title(&conn, "Night Signal");
    rate(&conn, &t, "star1");
    std::fs::create_dir_all(dir.path().join("new")).unwrap();
    std::fs::rename(&old, dir.path().join("new/a.flac")).unwrap();
    import(&conn, &dir.path().join("old"));

    let s = import(&conn, &dir.path().join("new"));
    assert_eq!(s.relinked, 1);
    assert_eq!(s.added, 0);
    assert!(playable_file(&conn, &t).unwrap().is_some());
    assert_eq!(rating_of(&conn, &t).as_deref(), Some("star1"));
}

#[test]
fn rescan_skips_unchanged_and_rereads_changed_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = copy_fixture("tone.wav", dir.path(), "a.wav");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let s = import(&conn, dir.path());
    assert_eq!((s.unchanged, s.updated), (1, 0));

    std::fs::copy(fixtures().join("other-stereo.flac"), &file).unwrap();
    let s = import(&conn, dir.path());
    assert_eq!(s.updated, 1);
}

#[test]
fn user_correction_survives_rescan_with_new_tags() {
    let dir = tempfile::tempdir().unwrap();
    let file = copy_fixture("tone.flac", dir.path(), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let t = track_by_title(&conn, "Night Signal");
    meta::set_correction(&conn, &t, Field::Title, Some("Night Signal (corrected)")).unwrap();

    // The file's tags change on disk (the user retagged it elsewhere).
    std::fs::copy(fixtures().join("unicode.flac"), &file).unwrap();
    import(&conn, dir.path());
    let m = meta::effective(&conn, &t).unwrap();
    assert_eq!(m.title.as_deref(), Some("Night Signal (corrected)"));
    assert_eq!(
        m.artist.as_deref(),
        Some("Róisín Mürphy"),
        "uncorrected fields follow the file"
    );
}

#[test]
fn identical_copies_share_a_track_and_primary_can_change() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("tone.flac", &dir.path().join("a"), "x.flac");
    copy_fixture("tone.flac", &dir.path().join("b"), "x.flac");
    let conn = crate::db::open_in_memory().unwrap();
    let s = import(&conn, dir.path());
    assert_eq!((s.added, s.duplicate_copies), (1, 1));
    let t = track_by_title(&conn, "Night Signal");
    let files = files_for_track(&conn, &t).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].is_primary && !files[1].is_primary);
    set_primary_file(&conn, &files[1].id).unwrap();
    let files = files_for_track(&conn, &t).unwrap();
    assert!(files[0].is_primary);
    assert_eq!(files.iter().filter(|f| f.is_primary).count(), 1);
}

#[test]
fn search_by_text_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    build_library(dir.path());
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());

    let q = |q: LibraryQuery| search(&conn, &q).unwrap();
    assert_eq!(q(LibraryQuery::default()).total, 6);
    let r = q(LibraryQuery {
        text: Some("roisin uber".into()),
        ..Default::default()
    });
    assert_eq!(r.total, 1);
    assert_eq!(r.rows[0].mix.as_deref(), Some("Dub"));

    assert_eq!(
        q(LibraryQuery {
            mix: Some("extended".into()),
            ..Default::default()
        })
        .total,
        2
    );
    assert_eq!(
        q(LibraryQuery {
            tempo_min: Some(120.0),
            tempo_max: Some(125.0),
            key: Some("8a".into()),
            ..Default::default()
        })
        .total,
        2
    );
    assert_eq!(
        q(LibraryQuery {
            availability: Some(Availability::Corrupt),
            ..Default::default()
        })
        .total,
        1
    );
    let t = track_by_title(&conn, "Four Forty");
    rate(&conn, &t, "star2");
    let r = q(LibraryQuery {
        rating: Some(RatingFilter::MinStars(2)),
        ..Default::default()
    });
    assert_eq!(r.total, 1);
    assert_eq!(r.rows[0].rating.as_deref(), Some("star2"));
    assert_eq!(
        q(LibraryQuery {
            rating: Some(RatingFilter::Unrated),
            ..Default::default()
        })
        .total,
        5
    );
}

#[test]
fn duplicate_suggestions_merge_and_dismiss() {
    let dir = tempfile::tempdir().unwrap();
    // Same tags, different encodings: a probable duplicate, not identical bytes.
    copy_fixture("tone.flac", dir.path(), "a.flac");
    copy_fixture("tone.mp3", dir.path(), "a.mp3");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let pairs = duplicates::suggestions(&conn, 10).unwrap();
    assert_eq!(pairs.len(), 1);
    let (keep, remove) = (pairs[0].track_a.clone(), pairs[0].track_b.clone());
    rate(&conn, &remove, "star3");
    add_to_playlist(&conn, &remove);

    assert!(
        duplicates::merge(&conn, &keep, &remove, " ").is_err(),
        "evidence is required"
    );
    duplicates::merge(&conn, &keep, &remove, "user confirmed duplicate in library").unwrap();
    assert_eq!(files_for_track(&conn, &keep).unwrap().len(), 2);
    assert_eq!(rating_of(&conn, &keep).as_deref(), Some("star3"));
    assert_eq!(playlist_entries(&conn, &keep), 1);
    assert_eq!(duplicates::resolve_track_id(&conn, &remove).unwrap(), keep);
    assert!(duplicates::suggestions(&conn, 10).unwrap().is_empty());
}

#[test]
fn dismissed_duplicates_are_not_suggested_again() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture("tone.flac", dir.path(), "a.flac");
    copy_fixture("tone.mp3", dir.path(), "a.mp3");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let p = &duplicates::suggestions(&conn, 10).unwrap()[0];
    duplicates::dismiss(&conn, &p.track_b, &p.track_a).unwrap();
    assert!(duplicates::suggestions(&conn, 10).unwrap().is_empty());
}

#[test]
fn check_files_notices_disappearing_and_returning_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = copy_fixture("tone.flac", dir.path(), "a.flac");
    let conn = crate::db::open_in_memory().unwrap();
    import(&conn, dir.path());
    let hidden = dir.path().join("away.bin");
    std::fs::rename(&file, &hidden).unwrap();
    assert_eq!(check_files(&conn, &RealProbe).unwrap().marked_missing, 1);
    std::fs::rename(&hidden, &file).unwrap();
    check_files(&conn, &RealProbe).unwrap();
    let t = track_by_title(&conn, "Night Signal");
    assert!(
        playable_file(&conn, &t).unwrap().is_some(),
        "reconnected drive restores the file"
    );
}

#[test]
fn import_job_runs_through_the_scheduler() {
    use crate::jobs::scheduler::{staged_bytes, Limits, Scheduler};
    use crate::jobs::worker::run_one;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    let dir = tempfile::tempdir().unwrap();
    build_library(dir.path());
    let mut conn = crate::db::open_in_memory().unwrap();
    let root = add_root(&conn, dir.path()).unwrap();
    let job_id = enqueue_import(&conn, &root).unwrap();
    let s = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    let h: Arc<dyn Handler> = Arc::new(ImportHandler {
        probe: Arc::new(RealProbe),
        after: None,
    });
    let handlers = HashMap::from([(h.kind(), h)]);
    assert!(run_one(
        &mut conn,
        &s,
        &handlers,
        &[kinds::IMPORT],
        "w",
        &AtomicBool::new(false)
    )
    .unwrap());
    let job = crate::jobs::get(&conn, &job_id).unwrap();
    assert_eq!(job.state, crate::domain::JobState::Done);
    let summary: ImportSummary = serde_json::from_value(job.checkpoint.unwrap()).unwrap();
    assert_eq!(summary.added, 6);
}

#[test]
fn import_of_unavailable_folder_fails_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_in_memory().unwrap();
    let sub = dir.path().join("drive");
    std::fs::create_dir(&sub).unwrap();
    let root = add_root(&conn, &sub).unwrap();
    std::fs::remove_dir(&sub).unwrap();
    match import_root(&conn, &root, &RealProbe, |_| Ok::<(), ()>(())) {
        Err(ImportError::Failed(Error::Invalid(m))) => assert!(m.contains("Connect its drive")),
        other => panic!("{other:?}"),
    }
}

/// Acceptance: library search works offline on a 10k-track library.
#[test]
fn search_is_fast_on_ten_thousand_tracks() {
    let conn = crate::db::open_in_memory().unwrap();
    let artists = [
        "Kerri Chandler",
        "Moodymann",
        "Theo Parrish",
        "Ron Trent",
        "Jayda G",
        "DJ Koze",
    ];
    let mixes = [
        None,
        Some("Original Mix"),
        Some("Dub"),
        Some("Extended Mix"),
        Some("Edit"),
    ];
    let keys = ["1A", "2A", "8A", "8B", "11B"];
    let tx = conn.unchecked_transaction().unwrap();
    for i in 0..10_000usize {
        let t = meta::create_track(&tx).unwrap();
        meta::set_extracted(
            &tx,
            &t,
            "tags",
            &[
                (Field::Artist, Some(artists[i % artists.len()].into())),
                (Field::Title, Some(format!("Track {i} Groove"))),
                (Field::Mix, mixes[i % mixes.len()].map(str::to_string)),
                (Field::Label, Some(format!("Label {}", i % 40))),
                (Field::Tempo, Some(format!("{}", 110 + i % 30))),
                (Field::MusicalKey, Some(keys[i % keys.len()].into())),
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash,
                                     is_primary, duration_ms, last_checked_at, created_at)
             VALUES (?1, ?2, ?3, 'imported', 1, 0, ?1, 1, 300000, 0, 0)",
            params![crate::util::new_id(), t, format!("/music/{i}.flac")],
        )
        .unwrap();
        if i % 7 == 0 {
            rate(&tx, &t, "star2");
        }
    }
    tx.commit().unwrap();

    let queries = [
        LibraryQuery {
            text: Some("chandler groove".into()),
            ..Default::default()
        },
        LibraryQuery {
            tempo_min: Some(122.0),
            tempo_max: Some(126.0),
            key: Some("8A".into()),
            ..Default::default()
        },
        LibraryQuery {
            rating: Some(RatingFilter::MinStars(2)),
            label: Some("Label 1".into()),
            ..Default::default()
        },
        LibraryQuery::default(),
    ];
    for q in &queries {
        // Warm once, then time.
        search(&conn, q).unwrap();
        let start = std::time::Instant::now();
        let page = search(&conn, q).unwrap();
        let elapsed = start.elapsed();
        assert!(page.total > 0);
        // Debug builds are several times slower than release; the target is
        // 50 ms in release. See docs/milestone-1-plan.md for measured values.
        let budget = if cfg!(debug_assertions) { 1000 } else { 50 };
        assert!(
            elapsed.as_millis() < budget,
            "query {q:?} took {elapsed:?} for {} results",
            page.total
        );
        eprintln!(
            "10k search: {:?} returned {} in {:?}",
            q.text.as_deref().unwrap_or("filters"),
            page.total,
            elapsed
        );
    }
}
