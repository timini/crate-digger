//! Playlists: ordered lists of tracks, stored by stable track ID.
//!
//! Playlists never own audio. Deleting a playlist removes its entries and
//! nothing else; tracks it referenced keep their files, ratings and other
//! memberships.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::domain::Availability;
use crate::util::{new_id, now_ms};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    /// What the playlist is for, in the user's words.
    pub brief: String,
    /// The app looks for tracks for this playlist in the background.
    pub discovery: bool,
    pub track_count: i64,
    pub duration_ms: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaylistEntry {
    pub position: i64,
    pub track_id: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub tempo: Option<f64>,
    pub musical_key: Option<String>,
    pub duration_ms: Option<i64>,
    pub path: Option<String>,
    pub availability: Option<Availability>,
    pub rating: Option<String>,
}

fn clean_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(Error::Invalid("a playlist needs a name".into()));
    }
    Ok(n.to_string())
}

fn touch(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE playlist SET updated_at = ?2 WHERE id = ?1",
        params![id, now_ms()],
    )?;
    if n == 0 {
        return Err(Error::NotFound(format!("playlist {id}")));
    }
    Ok(())
}

pub fn create(conn: &Connection, name: &str) -> Result<String> {
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO playlist (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![id, clean_name(name)?, now],
    )?;
    Ok(id)
}

pub fn rename(conn: &Connection, id: &str, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE playlist SET name = ?2 WHERE id = ?1",
        params![id, clean_name(name)?],
    )?;
    touch(conn, id)
}

/// Remove a playlist and its entries. Audio files and ratings are untouched.
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    let n = conn.execute("DELETE FROM playlist WHERE id = ?1", params![id])?;
    if n == 0 {
        return Err(Error::NotFound(format!("playlist {id}")));
    }
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.name, p.created_at, p.updated_at,
                (SELECT COUNT(*) FROM playlist_entry e WHERE e.playlist_id = p.id),
                (SELECT COALESCE(SUM(f.duration_ms), 0) FROM playlist_entry e
                   JOIN audio_file f ON f.track_id = e.track_id AND f.is_primary = 1
                  WHERE e.playlist_id = p.id),
                p.brief, p.discovery
         FROM playlist p ORDER BY p.name COLLATE NOCASE, p.created_at",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                track_count: r.get(4)?,
                duration_ms: r.get(5)?,
                brief: r.get(6)?,
                discovery: r.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn entries(conn: &Connection, id: &str) -> Result<Vec<PlaylistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT e.position, e.track_id, m.artist, m.title, m.mix, m.tempo, m.musical_key,
                f.duration_ms, f.path, f.availability, er.kind
         FROM playlist_entry e
         JOIN track_meta m ON m.track_id = e.track_id
         LEFT JOIN audio_file f ON f.id = (
             SELECT id FROM audio_file x WHERE x.track_id = e.track_id
             ORDER BY x.is_primary DESC, (x.availability = 'available') DESC, x.created_at LIMIT 1)
         LEFT JOIN effective_rating er ON er.track_id = e.track_id
         WHERE e.playlist_id = ?1
         ORDER BY e.position",
    )?;
    let rows = stmt
        .query_map(params![id], |r| {
            Ok(PlaylistEntry {
                position: r.get(0)?,
                track_id: r.get(1)?,
                artist: r.get(2)?,
                title: r.get(3)?,
                mix: r.get(4)?,
                tempo: r.get(5)?,
                musical_key: r.get(6)?,
                duration_ms: r.get(7)?,
                path: r.get(8)?,
                availability: r.get(9)?,
                rating: r.get(10)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn track_ids(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT track_id FROM playlist_entry WHERE playlist_id = ?1 ORDER BY position")?;
    let ids = stmt
        .query_map(params![id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(ids)
}

/// Replace a playlist's entries with `order`, numbering from zero.
/// Rewrites a playlist's entries. Joins the caller's transaction if one is
/// open, so it can be part of a larger change.
fn write_order(conn: &Connection, id: &str, order: &[String]) -> Result<()> {
    let own = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let tx: &Connection = own.as_deref().unwrap_or(conn);
    let added: Vec<(String, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT track_id, added_at FROM playlist_entry WHERE playlist_id = ?1 ORDER BY position",
        )?;
        let rows = stmt
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    tx.execute("DELETE FROM playlist_entry WHERE playlist_id = ?1", params![id])?;
    let now = now_ms();
    let mut used = vec![false; added.len()];
    for (pos, track) in order.iter().enumerate() {
        // Keep each entry's original added time when it survives a reorder.
        let added_at = added
            .iter()
            .enumerate()
            .find(|(i, (t, _))| !used[*i] && t == track)
            .map(|(i, (_, at))| {
                used[i] = true;
                *at
            })
            .unwrap_or(now);
        tx.execute(
            "INSERT INTO playlist_entry (playlist_id, position, track_id, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, pos as i64, track, added_at],
        )?;
    }
    touch(tx, id)?;
    if let Some(own) = own {
        own.commit()?;
    }
    Ok(())
}

/// Insert tracks at `at` (or the end). A track may appear more than once.
pub fn add_tracks(conn: &Connection, id: &str, tracks: &[String], at: Option<usize>) -> Result<()> {
    conn.query_row("SELECT 1 FROM playlist WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("playlist {id}")))?;
    for t in tracks {
        conn.query_row("SELECT 1 FROM track WHERE id = ?1", params![t], |_| Ok(()))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("track {t}")))?;
    }
    let mut order = track_ids(conn, id)?;
    let at = at.unwrap_or(order.len()).min(order.len());
    order.splice(at..at, tracks.iter().cloned());
    write_order(conn, id, &order)
}

pub fn remove_entry(conn: &Connection, id: &str, position: usize) -> Result<()> {
    let mut order = track_ids(conn, id)?;
    if position >= order.len() {
        return Err(Error::Invalid(format!("no entry at position {position}")));
    }
    order.remove(position);
    write_order(conn, id, &order)
}

/// Move the entry at `from` so it ends up at index `to`.
pub fn move_entry(conn: &Connection, id: &str, from: usize, to: usize) -> Result<()> {
    let mut order = track_ids(conn, id)?;
    if from >= order.len() || to >= order.len() {
        return Err(Error::Invalid("position out of range".into()));
    }
    let t = order.remove(from);
    order.insert(to, t);
    write_order(conn, id, &order)
}

/// Playlists that contain a track.
pub fn containing(conn: &Connection, track_id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT p.id, p.name FROM playlist p JOIN playlist_entry e ON e.playlist_id = p.id
         WHERE e.track_id = ?1 ORDER BY p.name COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map(params![track_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Whether a track's audio must be retained: it is in a playlist or the
/// user chose to keep it.
pub fn is_retained(conn: &Connection, track_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM playlist_entry WHERE track_id = ?1)
             OR EXISTS (SELECT 1 FROM keep_decision WHERE track_id = ?1)",
        params![track_id],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta;

    fn tracks(conn: &Connection, n: usize) -> Vec<String> {
        (0..n).map(|_| meta::create_track(conn).unwrap()).collect()
    }

    fn order(conn: &Connection, id: &str) -> Vec<String> {
        track_ids(conn, id).unwrap()
    }

    #[test]
    fn create_rename_list_delete() {
        let conn = crate::db::open_in_memory().unwrap();
        assert!(create(&conn, "  ").is_err());
        let id = create(&conn, " Warm-up ").unwrap();
        rename(&conn, &id, "Warm up").unwrap();
        let all = list(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Warm up");
        delete(&conn, &id).unwrap();
        assert!(list(&conn).unwrap().is_empty());
        assert!(delete(&conn, &id).is_err());
    }

    #[test]
    fn add_insert_move_and_remove_keep_positions_dense() {
        let conn = crate::db::open_in_memory().unwrap();
        let t = tracks(&conn, 4);
        let p = create(&conn, "Set").unwrap();
        add_tracks(&conn, &p, &t[..3], None).unwrap();
        add_tracks(&conn, &p, &t[3..], Some(1)).unwrap();
        assert_eq!(
            order(&conn, &p),
            vec![t[0].clone(), t[3].clone(), t[1].clone(), t[2].clone()]
        );

        move_entry(&conn, &p, 0, 3).unwrap();
        assert_eq!(
            order(&conn, &p),
            vec![t[3].clone(), t[1].clone(), t[2].clone(), t[0].clone()]
        );
        remove_entry(&conn, &p, 1).unwrap();
        let positions: Vec<i64> = entries(&conn, &p).unwrap().iter().map(|e| e.position).collect();
        assert_eq!(positions, vec![0, 1, 2]);
        assert!(move_entry(&conn, &p, 0, 9).is_err());
    }

    #[test]
    fn same_track_can_appear_twice() {
        let conn = crate::db::open_in_memory().unwrap();
        let t = tracks(&conn, 1);
        let p = create(&conn, "Loop").unwrap();
        add_tracks(&conn, &p, &[t[0].clone(), t[0].clone()], None).unwrap();
        assert_eq!(order(&conn, &p).len(), 2);
    }

    #[test]
    fn unknown_tracks_are_rejected() {
        let conn = crate::db::open_in_memory().unwrap();
        let p = create(&conn, "Set").unwrap();
        assert!(add_tracks(&conn, &p, &["nope".to_string()], None).is_err());
    }

    #[test]
    fn reorder_persists_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let (p, t) = {
            let conn = crate::db::open(&path).unwrap();
            let t = tracks(&conn, 3);
            let p = create(&conn, "Peak time").unwrap();
            add_tracks(&conn, &p, &t, None).unwrap();
            move_entry(&conn, &p, 2, 0).unwrap();
            (p, t)
        };
        let conn = crate::db::open(&path).unwrap();
        assert_eq!(order(&conn, &p), vec![t[2].clone(), t[0].clone(), t[1].clone()]);
    }

    #[test]
    fn deleting_a_playlist_leaves_files_ratings_and_other_playlists() {
        let conn = crate::db::open_in_memory().unwrap();
        let t = tracks(&conn, 1);
        conn.execute(
            "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash,
                                     is_primary, last_checked_at, created_at)
             VALUES ('f1', ?1, '/m/a.flac', 'imported', 1, 0, 'h', 1, 0, 0)",
            params![t[0]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rating_event (id, track_id, kind, session_id, created_at) VALUES ('r1', ?1, 'star2', 's', 1)",
            params![t[0]],
        )
        .unwrap();
        let a = create(&conn, "A").unwrap();
        let b = create(&conn, "B").unwrap();
        add_tracks(&conn, &a, &t, None).unwrap();
        add_tracks(&conn, &b, &t, None).unwrap();

        delete(&conn, &a).unwrap();
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM audio_file", [], |r| r.get(0))
            .unwrap();
        let ratings: i64 = conn
            .query_row("SELECT COUNT(*) FROM rating_event", [], |r| r.get(0))
            .unwrap();
        assert_eq!((files, ratings), (1, 1));
        assert_eq!(containing(&conn, &t[0]).unwrap().len(), 1);
        assert!(is_retained(&conn, &t[0]).unwrap());
        delete(&conn, &b).unwrap();
        assert!(!is_retained(&conn, &t[0]).unwrap());
    }
}
