//! Private backups: a snapshot of the user's ratings, keeps, seeds,
//! playlists and library metadata, and restoring one on this or another
//! computer. Audio files and credentials are never included.
//!
//! Restore finds each track by its file's content hash, then by artist,
//! title and mix. A track it cannot find is recreated without audio, with
//! its file marked missing, so the existing relink search can find the file
//! when the user points it at a folder.

use std::collections::HashMap;

use cd_protocol::backup::{FileRef, Playlist, Rating, Seed, Snapshot, Track, SNAPSHOT_VERSION};
use cd_protocol::Metadata;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::domain::{Field, RatingKind};
use crate::util::new_id;
use crate::{discovery, meta, playlists, review, Result};

pub const RESTORE_SESSION: &str = "restore";

fn file_ref(conn: &Connection, track_id: &str) -> Result<Option<FileRef>> {
    Ok(conn
        .query_row(
            "SELECT path, size_bytes, content_hash, duration_ms FROM audio_file WHERE track_id = ?1
             ORDER BY is_primary DESC, (availability = 'available') DESC, created_at LIMIT 1",
            params![track_id],
            |r| {
                let path: String = r.get(0)?;
                Ok(FileRef {
                    name: std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    size_bytes: r.get::<_, i64>(1)?.max(0) as u64,
                    content_hash: r.get(2)?,
                    duration_ms: r.get(3)?,
                })
            },
        )
        .optional()?
        .filter(|f| !f.name.is_empty()))
}

/// Everything the user has decided, as a snapshot.
pub fn snapshot(conn: &Connection, now: i64) -> Result<Snapshot> {
    // Tracks with audio in the library, a rating, a keep or a playlist place.
    let ids: Vec<String> = conn
        .prepare(
            "SELECT id FROM track t WHERE
                EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = t.id AND f.origin != 'staged')
             OR EXISTS (SELECT 1 FROM effective_rating r WHERE r.track_id = t.id)
             OR EXISTS (SELECT 1 FROM keep_decision k WHERE k.track_id = t.id)
             OR EXISTS (SELECT 1 FROM playlist_entry p WHERE p.track_id = t.id)
             ORDER BY id",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut tracks = vec![];
    for id in &ids {
        let m = meta::effective(conn, id)?;
        tracks.push(Track {
            id: id.clone(),
            metadata: Metadata {
                artist: m.artist,
                title: m.title,
                mix: m.mix,
                label: m.label,
                release: m.release,
                year: m.year.map(|y| y as i32),
                duration_ms: None,
            },
            fingerprint_hash: None,
            kept: review::is_kept(conn, id)?,
            file: file_ref(conn, id)?,
        });
    }
    let ratings = conn
        .prepare("SELECT track_id, kind, created_at FROM effective_rating ORDER BY created_at")?
        .query_map([], |r| {
            Ok(Rating {
                track: r.get(0)?,
                kind: r.get(1)?,
                at_ms: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let seeds = discovery::seeds(conn)?
        .into_iter()
        .map(|s| Seed {
            kind: s.kind.to_string(),
            value: s.value,
        })
        .collect();
    let mut lists = vec![];
    for p in playlists::list(conn)? {
        lists.push(Playlist {
            tracks: playlists::entries(conn, &p.id)?
                .into_iter()
                .map(|e| e.track_id)
                .collect(),
            name: p.name,
        });
    }
    Ok(Snapshot {
        version: SNAPSHOT_VERSION,
        created_at_ms: now,
        tracks,
        ratings,
        seeds,
        playlists: lists,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct RestoreSummary {
    /// Found in this library by file or by metadata.
    pub matched: usize,
    /// Recreated without audio; their files need relinking.
    pub to_relink: usize,
    pub ratings: usize,
    pub playlists: usize,
    pub seeds: usize,
}

fn find_local(conn: &Connection, t: &Track) -> Result<Option<String>> {
    if let Some(f) = &t.file {
        if let Some(id) = conn
            .query_row(
                "SELECT track_id FROM audio_file WHERE content_hash = ?1 AND size_bytes = ?2 LIMIT 1",
                params![f.content_hash, f.size_bytes as i64],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }
    }
    let m = &t.metadata;
    let (Some(artist), Some(title)) = (&m.artist, &m.title) else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT track_id FROM track_meta
             WHERE artist = ?1 COLLATE NOCASE AND title = ?2 COLLATE NOCASE
               AND COALESCE(mix, '') = COALESCE(?3, '') COLLATE NOCASE
             LIMIT 1",
            params![artist, title, m.mix],
            |r| r.get(0),
        )
        .optional()?)
}

fn recreate(conn: &Connection, t: &Track, now: i64) -> Result<String> {
    let id = meta::create_track(conn)?;
    let m = &t.metadata;
    meta::set_extracted(
        conn,
        &id,
        "backup",
        &[
            (Field::Artist, m.artist.clone()),
            (Field::Title, m.title.clone()),
            (Field::Mix, m.mix.clone()),
            (Field::Label, m.label.clone()),
            (Field::Release, m.release.clone()),
            (Field::Year, m.year.map(|y| y.to_string())),
        ],
    )?;
    if let Some(f) = &t.file {
        // A placeholder the relink search can match by hash, size or name.
        conn.execute(
            "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash, duration_ms,
                                     availability, availability_reason, is_primary, last_checked_at, created_at)
             VALUES (?1, ?2, ?3, 'imported', ?4, 0, ?5, ?6, 'missing',
                     'Restored from a backup. Use Find moved files to relink it.', 1, ?7, ?7)",
            params![
                new_id(),
                id,
                format!("(restored)/{}/{}", new_id(), f.name),
                f.size_bytes as i64,
                f.content_hash,
                f.duration_ms,
                now
            ],
        )?;
    }
    Ok(id)
}

/// Apply a snapshot to this library. Existing ratings are replaced only
/// where the snapshot differs, playlists that already exist get a suffix,
/// and nothing is deleted.
pub fn restore(conn: &Connection, s: &Snapshot, now: i64) -> Result<RestoreSummary> {
    use cd_protocol::Validate;
    s.validate()
        .map_err(|p| crate::Error::Invalid(format!("the backup is not valid: {}", p.message)))?;
    let tx = conn.unchecked_transaction()?;
    let mut summary = RestoreSummary::default();
    let mut local: HashMap<&str, String> = HashMap::new();
    for t in &s.tracks {
        let id = match find_local(&tx, t)? {
            Some(id) => {
                summary.matched += 1;
                id
            }
            None => {
                if t.file.is_some() {
                    summary.to_relink += 1;
                }
                recreate(&tx, t, now)?
            }
        };
        if t.kept {
            review::keep(&tx, &id, now)?;
        }
        local.insert(&t.id, id);
    }
    for r in &s.ratings {
        let (Some(id), Some(kind)) = (local.get(r.track.as_str()), RatingKind::parse(&r.kind)) else {
            continue;
        };
        if review::effective_rating(&tx, id)? != Some(kind) {
            // Appended to the log like any rating, so it can be undone.
            tx.execute(
                "INSERT INTO rating_event (id, track_id, kind, session_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_id(), id, kind, RESTORE_SESSION, now],
            )?;
            summary.ratings += 1;
        }
    }
    for seed in &s.seeds {
        summary.seeds += tx.execute(
            "INSERT OR IGNORE INTO seed (id, kind, value, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![new_id(), seed.kind, seed.value, now],
        )?;
    }
    tx.commit()?;
    // Playlists after the tracks exist; each is written in its own transaction.
    let existing: Vec<String> = playlists::list(conn)?.into_iter().map(|p| p.name).collect();
    for p in &s.playlists {
        let name = if existing.contains(&p.name) {
            format!("{} (restored)", p.name)
        } else {
            p.name.clone()
        };
        let pid = playlists::create(conn, &name)?;
        let ids: Vec<String> = p
            .tracks
            .iter()
            .filter_map(|t| local.get(t.as_str()).cloned())
            .collect();
        playlists::add_tracks(conn, &pid, &ids, None)?;
        summary.playlists += 1;
    }
    Ok(summary)
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
