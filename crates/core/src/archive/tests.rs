use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use super::*;
use crate::domain::Field;
use crate::real_probe::RealProbe;

fn m(
    artist: Option<&str>,
    release: Option<&str>,
    n: Option<&str>,
    title: Option<&str>,
    mix: Option<&str>,
) -> TrackMeta {
    TrackMeta {
        artist: artist.map(str::to_string),
        release: release.map(str::to_string),
        track_number: n.map(str::to_string),
        title: title.map(str::to_string),
        mix: mix.map(str::to_string),
        ..Default::default()
    }
}

fn p(s: &str) -> PathBuf {
    s.split('/').collect()
}

#[test]
fn template_follows_the_spec() {
    assert_eq!(
        relative_path(
            &m(
                Some("Kerri Chandler"),
                Some("Rain EP"),
                Some("3/4"),
                Some("Rain"),
                Some("Dub")
            ),
            "FLAC"
        ),
        p("Kerri Chandler/Rain EP/03 - Rain (Dub).flac")
    );
    // Missing release goes to Singles; missing number and mix are omitted.
    assert_eq!(
        relative_path(
            &m(Some("Moodymann"), None, None, Some("Shades"), Some(" ")),
            "mp3"
        ),
        p("Moodymann/Singles/Shades.mp3")
    );
    assert_eq!(
        relative_path(&m(None, None, None, None, None), "wav"),
        p("Unknown Artist/Singles/Untitled.wav")
    );
    // Vinyl positions are kept; the mix is not repeated.
    assert_eq!(
        relative_path(
            &m(Some("A"), Some("R"), Some("B2"), Some("Track (Dub)"), Some("Dub")),
            "mp3"
        ),
        p("A/R/B2 - Track (Dub).mp3")
    );
}

#[test]
fn sanitises_characters_that_break_on_some_platform() {
    assert_eq!(
        sanitize_component(r#"AC/DC: "Live" <at> \ the | bar? *"#, 100),
        "AC_DC_ _Live_ _at_ _ the _ bar_ _"
    );
    assert_eq!(sanitize_component("tab\there\nnewline", 100), "tab here newline");
    assert_eq!(sanitize_component("Trailing dots...", 100), "Trailing dots");
    assert_eq!(sanitize_component("Trailing space   ", 100), "Trailing space");
    assert_eq!(sanitize_component(".hidden", 100), "_.hidden");
    assert_eq!(sanitize_component("..", 100), "_");
    assert_eq!(sanitize_component("", 100), "_");
    assert_eq!(sanitize_component("   ", 100), "_");
}

#[test]
fn avoids_windows_reserved_names() {
    for name in ["CON", "con", "PRN", "AUX", "NUL", "COM1", "lpt9", "COM¹"] {
        let s = sanitize_component(name, 100);
        assert_ne!(s.to_uppercase(), name.to_uppercase(), "{name}");
        assert!(s.ends_with('_'), "{name} -> {s}");
    }
    assert_eq!(sanitize_component("NUL.txt", 100), "NUL_.txt");
    assert_eq!(
        sanitize_component("Console", 100),
        "Console",
        "only exact names are reserved"
    );
}

#[test]
fn normalises_unicode_and_limits_length_without_splitting_characters() {
    // "é" as e + combining accent becomes the single composed character.
    assert_eq!(sanitize_component("Cafe\u{301}", 100), "Café");
    let long = "é".repeat(200);
    let s = sanitize_component(&long, 150);
    assert!(s.len() <= 150);
    assert!(s.chars().all(|c| c == 'é'));
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures")
}

struct Setup {
    // Fail points are global: every test that moves files holds this lock so
    // a crash test's fail point cannot fire inside another test.
    _failpoints: fail::FailScenario<'static>,
    _dir: tempfile::TempDir,
    db: PathBuf,
    staging: PathBuf,
    cfg: ArchiveConfig,
}

fn setup(force_copy: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let staging = dir.path().join("staging");
    let cfg = ArchiveConfig {
        root: dir.path().join("archive"),
        force_copy,
    };
    Setup {
        _failpoints: fail::FailScenario::setup(),
        db,
        staging,
        cfg,
        _dir: dir,
    }
}

/// A staged copy of the FLAC fixture on a new track. Returns (track, file).
fn staged(conn: &Connection, staging: &Path, name: &str) -> (String, String) {
    let t = meta::create_track(conn).unwrap();
    meta::set_extracted(
        conn,
        &t,
        "test",
        &[
            (Field::Artist, Some("Fixture Collective".into())),
            (Field::Title, Some("Night Signal".into())),
            (Field::Mix, Some("Dub".into())),
        ],
    )
    .unwrap();
    let dir = staging.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("download.flac");
    std::fs::copy(fixtures().join("tone.flac"), &path).unwrap();
    let r = library::register_staged(conn, &path, &t, &RealProbe).unwrap();
    (t, r.file_id)
}

fn original_hash() -> blake3::Hash {
    fsops::hash_file(&fixtures().join("tone.flac")).unwrap()
}

fn file_path(conn: &Connection, file_id: &str) -> (PathBuf, String) {
    conn.query_row(
        "SELECT path, origin FROM audio_file WHERE id = ?1",
        params![file_id],
        |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get(1)?)),
    )
    .unwrap()
}

#[test]
fn keep_promotes_into_the_archive() {
    let s = setup(false);
    let conn = crate::db::open(&s.db).unwrap();
    let (t, f) = staged(&conn, &s.staging, "c1");
    let jobs = keep_track(&conn, &t, 1).unwrap();
    assert_eq!(jobs.len(), 1);
    let dest = promote(&conn, &f, &s.cfg, false).unwrap();
    // The release and number come from the file's own tags, since the
    // source did not supply them.
    assert_eq!(
        dest,
        s.cfg.root.join(p(
            "Fixture Collective/Test Pressings EP/03 - Night Signal (Dub).flac"
        ))
    );
    assert_eq!(fsops::hash_file(&dest).unwrap(), original_hash());
    let (path, origin) = file_path(&conn, &f);
    assert_eq!((path, origin.as_str()), (dest.clone(), "archived"));
    assert!(!s.staging.join("c1").exists(), "empty staging folder is tidied");
    // Idempotent.
    assert_eq!(promote(&conn, &f, &s.cfg, false).unwrap(), dest);
}

#[test]
fn collisions_never_overwrite() {
    let s = setup(false);
    let conn = crate::db::open(&s.db).unwrap();
    let planned = s.cfg.root.join(p(
        "Fixture Collective/Test Pressings EP/03 - Night Signal (Dub).flac",
    ));
    std::fs::create_dir_all(planned.parent().unwrap()).unwrap();
    std::fs::write(&planned, b"someone else's file").unwrap();

    let (_, f1) = staged(&conn, &s.staging, "c1");
    let (_, f2) = staged(&conn, &s.staging, "c2");
    let d1 = promote(&conn, &f1, &s.cfg, false).unwrap();
    let d2 = promote(&conn, &f2, &s.cfg, false).unwrap();
    assert_eq!(std::fs::read(&planned).unwrap(), b"someone else's file");
    assert!(
        d1.ends_with("03 - Night Signal (Dub) (2).flac"),
        "{}",
        d1.display()
    );
    assert!(
        d2.ends_with("03 - Night Signal (Dub) (3).flac"),
        "{}",
        d2.display()
    );
}

#[test]
fn imported_files_stay_put_unless_managed() {
    let s = setup(false);
    let conn = crate::db::open(&s.db).unwrap();
    let dir = s.staging.parent().unwrap().join("music");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mine.flac");
    std::fs::copy(fixtures().join("tone.flac"), &path).unwrap();
    let r = library::register_file(&conn, &path, None, FileOrigin::Imported, &RealProbe).unwrap();
    assert!(promote(&conn, &r.file_id, &s.cfg, false).is_err());
    assert!(path.exists());
    let dest = promote(&conn, &r.file_id, &s.cfg, true).unwrap();
    assert!(dest.exists() && !path.exists());
}

/// Crash at a fail point during promotion, reopen, recover, and check that
/// exactly one verified copy exists and the database points at it.
fn crash_case(point: &str, force_copy: bool) {
    let s = setup(force_copy);
    let file_id = {
        let conn = crate::db::open(&s.db).unwrap();
        let (_, f) = staged(&conn, &s.staging, "c1");
        fail::cfg(point, "panic").unwrap();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| promote(&conn, &f, &s.cfg, false)));
        assert!(r.is_err(), "{point} did not fire");
        fail::remove(point);
        f
    };

    let conn = crate::db::open(&s.db).unwrap();
    let rec = recover(&conn, &s.cfg).unwrap();
    assert_eq!(rec.finished, 1, "{point}: {:?}", rec.failed);

    let (path, origin) = file_path(&conn, &file_id);
    assert_eq!(origin, "archived", "{point}");
    assert!(path.starts_with(&s.cfg.root));
    assert_eq!(
        fsops::hash_file(&path).unwrap(),
        original_hash(),
        "{point}: audio damaged"
    );
    let staged_copy = s.staging.join("c1/download.flac");
    assert!(!staged_copy.exists(), "{point}: staging copy left behind");
    let parts: Vec<_> = walkdir::WalkDir::new(&s.cfg.root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().to_string_lossy().ends_with(fsops::PART_SUFFIX))
        .collect();
    assert!(parts.is_empty(), "{point}: partial copy left behind");
}

#[test]
fn crash_after_rename_before_record() {
    crash_case("archive.after_move", false);
}

#[test]
fn crash_after_copy_before_record() {
    crash_case("archive.after_move", true);
}

#[test]
fn crash_before_database_update() {
    crash_case("archive.before_db", false);
    crash_case("archive.before_db", true);
}

#[test]
fn crash_after_database_update_before_source_removal() {
    crash_case("archive.after_db", true);
    crash_case("archive.after_db", false);
}

#[test]
fn crash_during_copy_leaves_source_and_no_partial() {
    let s = setup(true);
    let conn = crate::db::open(&s.db).unwrap();
    let (_, f) = staged(&conn, &s.staging, "c1");
    // Simulate a copy that died halfway: an op in 'intent' and a partial file.
    let (src, _) = file_path(&conn, &f);
    let dest = s.cfg.root.join("A/Singles/X.flac");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(fsops::part_path(&dest), b"half a file").unwrap();
    conn.execute(
        "INSERT INTO archive_op (id, audio_file_id, src_path, dest_path, step, created_at, updated_at)
         VALUES ('op', ?1, ?2, ?3, 'intent', 0, 0)",
        params![f, library::path_to_db(&src), library::path_to_db(&dest)],
    )
    .unwrap();
    recover(&conn, &s.cfg).unwrap();
    assert!(!fsops::part_path(&dest).exists());
    assert_eq!(fsops::hash_file(&dest).unwrap(), original_hash());
    assert!(!src.exists());
}

#[test]
fn a_file_that_appears_at_the_destination_is_never_overwritten() {
    let s = setup(false);
    let conn = crate::db::open(&s.db).unwrap();
    let (_, f) = staged(&conn, &s.staging, "c1");
    let (src, _) = file_path(&conn, &f);
    let dest = s.cfg.root.join("A/Singles/X.flac");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, b"not ours").unwrap();
    conn.execute(
        "INSERT INTO archive_op (id, audio_file_id, src_path, dest_path, step, created_at, updated_at)
         VALUES ('op', ?1, ?2, ?3, 'intent', 0, 0)",
        params![f, library::path_to_db(&src), library::path_to_db(&dest)],
    )
    .unwrap();
    let rec = recover(&conn, &s.cfg).unwrap();
    assert_eq!(rec.failed.len(), 1);
    assert_eq!(std::fs::read(&dest).unwrap(), b"not ours");
    assert!(src.exists());
    assert_eq!(file_path(&conn, &f).1, "staged");
}

#[test]
fn clearing_temporary_audio_keeps_metadata_features_and_retained_tracks() {
    let s = setup(false);
    let conn = crate::db::open(&s.db).unwrap();
    let (reviewed, rf) = staged(&conn, &s.staging, "reviewed");
    let (unreviewed, _) = staged(&conn, &s.staging, "unreviewed");
    let (kept, _) = staged(&conn, &s.staging, "kept");
    let (listed, _) = staged(&conn, &s.staging, "listed");
    for (t, stage) in [
        (&reviewed, "reviewed"),
        (&unreviewed, "ready"),
        (&kept, "reviewed"),
        (&listed, "reviewed"),
    ] {
        conn.execute(
            "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES (?1, ?1 || '-c', ?2, 0, 0)",
            params![t, stage],
        )
        .ok();
        conn.execute(
            "INSERT INTO candidate (id, track_id, stage, created_at, updated_at) VALUES (?1, ?2, ?3, 0, 0)",
            params![format!("c-{t}"), t, stage],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO feature_record (id, track_id, audio_file_id, model_id, weights_checksum, preprocessing_version,
             source_fingerprint, segment_start_ms, segment_end_ms, dims, created_at)
         VALUES ('fr', ?1, ?2, 'm', 'w', 'v', 'fp', 0, 1, 0, 0)",
        params![reviewed, rf],
    )
    .unwrap();
    crate::review::keep(&conn, &kept, 1).unwrap();
    let pl = playlists::create(&conn, "Set").unwrap();
    playlists::add_tracks(&conn, &pl, std::slice::from_ref(&listed), None).unwrap();

    let out = clear_temporary(&conn, false).unwrap();
    assert_eq!(
        (out.removed_files, out.retained, out.unreviewed_skipped),
        (1, 2, 1)
    );
    assert!(out.freed_bytes > 0);
    assert!(!s.staging.join("reviewed").exists());
    assert_eq!(
        meta::effective(&conn, &reviewed).unwrap().title.as_deref(),
        Some("Night Signal")
    );
    let feature_file: Option<String> = conn
        .query_row(
            "SELECT audio_file_id FROM feature_record WHERE id = 'fr'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(feature_file, None, "feature kept, detached from the deleted file");

    // Explicitly including unreviewed audio clears it too, and says why.
    let out = clear_temporary(&conn, true).unwrap();
    assert_eq!(out.removed_files, 1);
    let reason: String = conn
        .query_row(
            "SELECT status_reason FROM candidate WHERE track_id = ?1",
            params![unreviewed],
            |r| r.get(0),
        )
        .unwrap();
    assert!(reason.contains("cleared"));
    // Kept and playlisted audio is still there.
    assert!(s.staging.join("kept/download.flac").exists());
    assert!(s.staging.join("listed/download.flac").exists());
}
