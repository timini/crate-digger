//! Library import and file management.
//!
//! Import indexes files where they are. It never moves, renames or writes
//! to them: tags are read with a read-only parser and files are only opened
//! for reading.

pub mod duplicates;
pub mod search;
pub mod tags;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::{Availability, FileOrigin};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::util::{new_id, now_ms};
use crate::{meta, Error, Result};

/// Checks that a file decodes. Implemented by the audio crate; tests use a
/// fake. Returns a user-facing explanation on failure.
pub trait AudioProbe: Send + Sync {
    /// Open the file and decode its first packets.
    fn probe(&self, path: &Path) -> std::result::Result<ProbeInfo, String>;
    fn is_supported(&self, path: &Path) -> bool;
    /// Decode the whole file; returns the decoded length in milliseconds.
    fn decode_full(&self, path: &Path) -> std::result::Result<i64, String>;
    /// Peak overview scaled to 0..=255.
    fn waveform(&self, path: &Path, bins: usize) -> std::result::Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeInfo {
    pub codec: String,
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
}

// ---------------------------------------------------------------------------
// Content hashing
// ---------------------------------------------------------------------------

const SAMPLE: u64 = 1024 * 1024;

/// A fingerprint of the file bytes: BLAKE3 over the size, the first MiB and
/// the last MiB (or the whole file when it is under 2 MiB). It is cheap
/// enough for large libraries and identifies byte-identical copies and moved
/// files. It is not an audio fingerprint: re-encoded or re-tagged copies
/// hash differently.
pub fn content_hash(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let size = f.metadata().map_err(|e| Error::io(path, e))?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buf = Vec::new();
    if size <= 2 * SAMPLE {
        f.read_to_end(&mut buf).map_err(|e| Error::io(path, e))?;
        hasher.update(&buf);
    } else {
        buf.resize(SAMPLE as usize, 0);
        f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
        hasher.update(&buf);
        f.seek(SeekFrom::End(-(SAMPLE as i64)))
            .map_err(|e| Error::io(path, e))?;
        f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
        hasher.update(&buf);
    }
    Ok(format!("b3s:{}", hasher.finalize().to_hex()))
}

fn stat(path: &Path) -> std::io::Result<(i64, i64)> {
    let m = std::fs::metadata(path)?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok((m.len() as i64, mtime))
}

/// Paths are stored as given by the OS, with no normalisation, so that the
/// exact file can always be reopened.
pub fn path_to_db(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn missing_reason(path: &str) -> String {
    format!(
        "Not found at {path}. It may have been moved, renamed or deleted, or its drive may be disconnected. \
         Use Relink to find it."
    )
}

fn corrupt_reason(detail: &str) -> String {
    format!("Could not decode this file ({detail}). It may be damaged or incomplete; replace it with a good copy.")
}

// ---------------------------------------------------------------------------
// Roots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryRoot {
    pub id: String,
    pub path: String,
    pub created_at: i64,
    pub file_count: i64,
}

pub fn add_root(conn: &Connection, path: &Path) -> Result<String> {
    if !path.is_dir() {
        return Err(Error::Invalid(format!("{} is not a folder", path.display())));
    }
    let p = path_to_db(path);
    if let Some(id) = conn
        .query_row("SELECT id FROM library_root WHERE path = ?1", params![p], |r| {
            r.get(0)
        })
        .optional()?
    {
        return Ok(id);
    }
    let id = new_id();
    conn.execute(
        "INSERT INTO library_root (id, path, created_at) VALUES (?1, ?2, ?3)",
        params![id, p, now_ms()],
    )?;
    Ok(id)
}

/// Forget a folder. Its tracks, ratings and playlists stay; files simply
/// stop being rescanned.
pub fn remove_root(conn: &Connection, root_id: &str) -> Result<()> {
    conn.execute("DELETE FROM library_root WHERE id = ?1", params![root_id])?;
    Ok(())
}

pub fn roots(conn: &Connection) -> Result<Vec<LibraryRoot>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.path, r.created_at,
                (SELECT COUNT(*) FROM audio_file f WHERE f.library_root_id = r.id)
         FROM library_root r ORDER BY r.path",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LibraryRoot {
                id: r.get(0)?,
                path: r.get(1)?,
                created_at: r.get(2)?,
                file_count: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Queue a scan of a library folder.
pub fn enqueue_import(conn: &Connection, root_id: &str) -> Result<String> {
    // One scan per root per request; a finished scan can be requested again.
    let key = format!("import:{root_id}:{}", new_id());
    Ok(crate::jobs::enqueue(
        conn,
        &NewJob::new(kinds::IMPORT, key, serde_json::json!({ "root_id": root_id })),
        now_ms(),
    )?
    .id)
}

// ---------------------------------------------------------------------------
// Registering files
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterOutcome {
    /// A new track was created for this file.
    Added,
    /// The file changed since the last scan and was re-read.
    Updated,
    Unchanged,
    /// Byte-identical to a file already in the library; attached to the
    /// same track as an extra copy.
    DuplicateCopy,
    /// A missing file turned up at a new path and was relinked.
    Relinked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Registered {
    pub file_id: String,
    pub track_id: String,
    pub outcome: RegisterOutcome,
    pub availability: Availability,
}

struct FileFacts {
    size: i64,
    mtime: i64,
    hash: String,
    tags: Option<tags::FileTags>,
    probe: std::result::Result<ProbeInfo, String>,
}

fn gather(path: &Path, probe: &dyn AudioProbe) -> Result<FileFacts> {
    let (size, mtime) = stat(path).map_err(|e| Error::io(path, e))?;
    let hash = content_hash(path)?;
    let tags = tags::read(path).ok();
    let probe = probe.probe(path);
    Ok(FileFacts {
        size,
        mtime,
        hash,
        tags,
        probe,
    })
}

fn availability_of(facts: &FileFacts) -> (Availability, Option<String>) {
    match &facts.probe {
        Ok(_) => (Availability::Available, None),
        Err(e) => (Availability::Corrupt, Some(corrupt_reason(e))),
    }
}

fn write_file_facts(conn: &Connection, file_id: &str, path: &str, facts: &FileFacts) -> Result<()> {
    let (availability, reason) = availability_of(facts);
    let probe = facts.probe.as_ref().ok();
    let t = facts.tags.as_ref();
    let duration = t
        .and_then(|t| t.duration_ms)
        .or_else(|| probe.and_then(|p| p.duration_ms));
    conn.execute(
        "UPDATE audio_file SET path = ?2, size_bytes = ?3, mtime_ms = ?4, content_hash = ?5,
                duration_ms = ?6, format = ?7, sample_rate = ?8, channels = ?9, bitrate_kbps = ?10,
                availability = ?11, availability_reason = ?12, waveform = NULL, last_checked_at = ?13
         WHERE id = ?1",
        params![
            file_id,
            path,
            facts.size,
            facts.mtime,
            facts.hash,
            duration,
            probe.map(|p| p.codec.clone()),
            t.and_then(|t| t.sample_rate)
                .or_else(|| probe.and_then(|p| p.sample_rate)),
            t.and_then(|t| t.channels)
                .or_else(|| probe.and_then(|p| p.channels)),
            t.and_then(|t| t.bitrate_kbps),
            availability,
            reason,
            now_ms()
        ],
    )?;
    Ok(())
}

fn apply_tags(conn: &Connection, track_id: &str, facts: &FileFacts) -> Result<()> {
    if let Some(t) = &facts.tags {
        meta::set_extracted(conn, track_id, "tags", &t.fields())?;
    }
    Ok(())
}

/// Index one file. Safe to call repeatedly; unchanged files are skipped
/// after a `stat`.
pub fn register_file(
    conn: &Connection,
    path: &Path,
    root_id: Option<&str>,
    origin: FileOrigin,
    probe: &dyn AudioProbe,
) -> Result<Registered> {
    let p = path_to_db(path);
    let existing: Option<(String, String, i64, i64, Availability, bool)> = conn
        .query_row(
            "SELECT id, track_id, size_bytes, mtime_ms, availability, is_primary FROM audio_file WHERE path = ?1",
            params![p],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;

    if let Some((file_id, track_id, size, mtime, availability, is_primary)) = existing {
        let (now_size, now_mtime) = stat(path).map_err(|e| Error::io(path, e))?;
        if now_size == size && now_mtime == mtime && availability != Availability::Missing {
            return Ok(Registered {
                file_id,
                track_id,
                outcome: RegisterOutcome::Unchanged,
                availability,
            });
        }
        let facts = gather(path, probe)?;
        write_file_facts(conn, &file_id, &p, &facts)?;
        // Only the primary copy's tags describe the track.
        if is_primary {
            apply_tags(conn, &track_id, &facts)?;
        }
        return Ok(Registered {
            file_id,
            track_id,
            outcome: RegisterOutcome::Updated,
            availability: availability_of(&facts).0,
        });
    }

    let facts = gather(path, probe)?;
    let same_bytes: Vec<(String, String, Availability, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, track_id, availability, path FROM audio_file
             WHERE content_hash = ?1 AND size_bytes = ?2 ORDER BY is_primary DESC, created_at",
        )?;
        let rows = stmt
            .query_map(params![facts.hash, facts.size], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };

    // A missing file with identical bytes has moved here: relink it, keeping
    // its track, ratings and playlist entries.
    let missing: Vec<_> = same_bytes
        .iter()
        .filter(|(_, _, a, _)| *a == Availability::Missing)
        .collect();
    if missing.len() == 1 {
        let (file_id, track_id, _, old_path) = missing[0];
        write_file_facts(conn, file_id, &p, &facts)?;
        if root_id.is_some() {
            conn.execute(
                "UPDATE audio_file SET library_root_id = ?2 WHERE id = ?1",
                params![file_id, root_id],
            )?;
        }
        tracing::info!(from = %old_path, to = %p, "relinked moved file by content");
        return Ok(Registered {
            file_id: file_id.clone(),
            track_id: track_id.clone(),
            outcome: RegisterOutcome::Relinked,
            availability: availability_of(&facts).0,
        });
    }

    let (track_id, outcome, primary) = match same_bytes.first() {
        Some((_, track_id, _, _)) => (track_id.clone(), RegisterOutcome::DuplicateCopy, false),
        None => (meta::create_track(conn)?, RegisterOutcome::Added, true),
    };
    let file_id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO audio_file (id, track_id, path, origin, library_root_id, size_bytes, mtime_ms,
                                 content_hash, is_primary, last_checked_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        params![
            file_id,
            track_id,
            p,
            origin,
            root_id,
            facts.size,
            facts.mtime,
            facts.hash,
            primary,
            now
        ],
    )?;
    write_file_facts(conn, &file_id, &p, &facts)?;
    if outcome == RegisterOutcome::Added {
        apply_tags(conn, &track_id, &facts)?;
    }
    Ok(Registered {
        file_id,
        track_id,
        outcome,
        availability: availability_of(&facts).0,
    })
}

/// Register a downloaded file in staging as a copy of `track_id`. The
/// file's own tags are stored at low priority: the candidate's identified
/// metadata stays in charge. Safe to call again for the same path.
pub fn register_staged(
    conn: &Connection,
    path: &Path,
    track_id: &str,
    probe: &dyn AudioProbe,
) -> Result<Registered> {
    let p = path_to_db(path);
    if let Some((file_id, existing_track, availability)) = conn
        .query_row(
            "SELECT id, track_id, availability FROM audio_file WHERE path = ?1",
            params![p],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?)),
        )
        .optional()?
    {
        if existing_track != track_id {
            return Err(Error::Conflict(format!(
                "{p} is already registered to another track"
            )));
        }
        return Ok(Registered {
            file_id,
            track_id: existing_track,
            outcome: RegisterOutcome::Unchanged,
            availability,
        });
    }
    let facts = gather(path, probe)?;
    let has_primary: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM audio_file WHERE track_id = ?1 AND is_primary = 1)",
        params![track_id],
        |r| r.get(0),
    )?;
    let file_id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO audio_file (id, track_id, path, origin, size_bytes, mtime_ms, content_hash, is_primary,
                                 last_checked_at, created_at)
         VALUES (?1, ?2, ?3, 'staged', ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            file_id,
            track_id,
            p,
            facts.size,
            facts.mtime,
            facts.hash,
            !has_primary,
            now
        ],
    )?;
    write_file_facts(conn, &file_id, &p, &facts)?;
    if let Some(t) = &facts.tags {
        meta::set_extracted(conn, track_id, "download_tags", &t.fields())?;
    }
    Ok(Registered {
        file_id,
        track_id: track_id.to_string(),
        outcome: RegisterOutcome::Added,
        availability: availability_of(&facts).0,
    })
}

// ---------------------------------------------------------------------------
// Scanning a root
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportSummary {
    pub total: usize,
    pub processed: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub duplicate_copies: usize,
    pub relinked: usize,
    pub corrupt: usize,
    pub marked_missing: usize,
    pub errors: Vec<String>,
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

/// List supported audio files under `dir`, sorted, skipping hidden files
/// and folders (including macOS `._` resource forks).
pub fn list_audio_files(dir: &Path, probe: &dyn AudioProbe) -> Vec<PathBuf> {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && probe.is_supported(e.path()))
        .map(|e| e.into_path())
        .collect()
}

/// Scan a library root. `progress` is called after every batch; returning
/// an error stops the scan (used for pause and quit).
pub fn import_root<E>(
    conn: &Connection,
    root_id: &str,
    probe: &dyn AudioProbe,
    mut progress: impl FnMut(&ImportSummary) -> std::result::Result<(), E>,
) -> std::result::Result<ImportSummary, ImportError<E>> {
    let root: String = conn
        .query_row(
            "SELECT path FROM library_root WHERE id = ?1",
            params![root_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(Error::from)?
        .ok_or_else(|| Error::NotFound(format!("library folder {root_id}")))?;
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(Error::Invalid(format!(
            "The folder {root} is not available. Connect its drive or remove it from Settings."
        ))
        .into());
    }

    let files = list_audio_files(&root_path, probe);
    let mut summary = ImportSummary {
        total: files.len(),
        ..Default::default()
    };
    let mut seen = std::collections::HashSet::new();

    // Batches keep the number of fsyncs down on large libraries.
    for batch in files.chunks(200) {
        let tx = conn.unchecked_transaction().map_err(Error::from)?;
        for path in batch {
            seen.insert(path_to_db(path));
            match register_file(&tx, path, Some(root_id), FileOrigin::Imported, probe) {
                Ok(r) => {
                    match r.outcome {
                        RegisterOutcome::Added => summary.added += 1,
                        RegisterOutcome::Updated => summary.updated += 1,
                        RegisterOutcome::Unchanged => summary.unchanged += 1,
                        RegisterOutcome::DuplicateCopy => summary.duplicate_copies += 1,
                        RegisterOutcome::Relinked => summary.relinked += 1,
                    }
                    if r.availability == Availability::Corrupt && r.outcome != RegisterOutcome::Unchanged {
                        summary.corrupt += 1;
                    }
                }
                Err(e) => {
                    if summary.errors.len() < 50 {
                        summary.errors.push(format!("{}: {e}", path.display()));
                    }
                }
            }
            summary.processed += 1;
        }
        tx.commit().map_err(Error::from)?;
        progress(&summary).map_err(ImportError::Stopped)?;
    }

    // Files previously indexed under this root that were not found.
    let mut stmt = conn
        .prepare(
            "SELECT id, path FROM audio_file
             WHERE library_root_id = ?1 AND availability != 'missing'",
        )
        .map_err(Error::from)?;
    let known: Vec<(String, String)> = stmt
        .query_map(params![root_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(Error::from)?
        .collect::<std::result::Result<_, _>>()
        .map_err(Error::from)?;
    for (id, path) in known {
        if !seen.contains(&path) && !Path::new(&path).exists() {
            mark_missing(conn, &id, &path)?;
            summary.marked_missing += 1;
        }
    }
    Ok(summary)
}

#[derive(Debug)]
pub enum ImportError<E> {
    Failed(Error),
    Stopped(E),
}

impl<E> From<Error> for ImportError<E> {
    fn from(e: Error) -> Self {
        ImportError::Failed(e)
    }
}

pub fn mark_missing(conn: &Connection, file_id: &str, path: &str) -> Result<()> {
    conn.execute(
        "UPDATE audio_file SET availability = 'missing', availability_reason = ?2, last_checked_at = ?3
         WHERE id = ?1",
        params![file_id, missing_reason(path), now_ms()],
    )?;
    Ok(())
}

/// Record that a file could not be decoded (for example when playback
/// fails). Metadata and ratings are untouched.
pub fn mark_corrupt(conn: &Connection, file_id: &str, detail: &str) -> Result<()> {
    conn.execute(
        "UPDATE audio_file SET availability = 'corrupt', availability_reason = ?2, last_checked_at = ?3
         WHERE id = ?1",
        params![file_id, corrupt_reason(detail), now_ms()],
    )?;
    Ok(())
}

/// Re-check every known file: missing files that reappeared become
/// available, changed files are re-read, vanished files become missing.
pub fn check_files(conn: &Connection, probe: &dyn AudioProbe) -> Result<ImportSummary> {
    let files: Vec<(String, String, Option<String>, FileOrigin)> = {
        let mut stmt = conn.prepare("SELECT id, path, library_root_id, origin FROM audio_file")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let mut summary = ImportSummary {
        total: files.len(),
        ..Default::default()
    };
    for (id, path, root, origin) in files {
        let p = Path::new(&path);
        if p.exists() {
            match register_file(conn, p, root.as_deref(), origin, probe) {
                Ok(r) if r.outcome == RegisterOutcome::Updated => summary.updated += 1,
                Ok(_) => summary.unchanged += 1,
                Err(e) => summary.errors.push(format!("{path}: {e}")),
            }
        } else {
            let was: Availability = conn.query_row(
                "SELECT availability FROM audio_file WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            if was != Availability::Missing {
                mark_missing(conn, &id, &path)?;
                summary.marked_missing += 1;
            }
        }
        summary.processed += 1;
    }
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Relinking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelinkProposal {
    pub file_id: String,
    pub track_id: String,
    pub old_path: String,
    pub new_path: String,
    /// `content` (identical bytes) or `name_and_duration`.
    pub method: String,
}

/// Point a file record at a new location chosen by the user.
pub fn relink_file(conn: &Connection, file_id: &str, new_path: &Path, probe: &dyn AudioProbe) -> Result<()> {
    if !new_path.is_file() {
        return Err(Error::Invalid(format!("{} is not a file", new_path.display())));
    }
    let p = path_to_db(new_path);
    let taken: Option<String> = conn
        .query_row("SELECT id FROM audio_file WHERE path = ?1", params![p], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(other) = taken {
        if other != file_id {
            return Err(Error::Conflict(format!(
                "{} is already in the library as another file",
                new_path.display()
            )));
        }
    }
    let facts = gather(new_path, probe)?;
    write_file_facts(conn, file_id, &p, &facts)
}

/// Look in `folder` for files that match missing library files.
pub fn find_moved(conn: &Connection, folder: &Path, probe: &dyn AudioProbe) -> Result<Vec<RelinkProposal>> {
    struct Missing {
        id: String,
        track_id: String,
        path: String,
        size: i64,
        hash: String,
        duration: Option<i64>,
    }
    let missing: Vec<Missing> = {
        let mut stmt = conn.prepare(
            "SELECT id, track_id, path, size_bytes, content_hash, duration_ms FROM audio_file
             WHERE availability = 'missing'",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Missing {
                    id: r.get(0)?,
                    track_id: r.get(1)?,
                    path: r.get(2)?,
                    size: r.get(3)?,
                    hash: r.get(4)?,
                    duration: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let mut proposals = Vec::new();
    let mut matched = std::collections::HashSet::new();
    for candidate in list_audio_files(folder, probe) {
        let cp = path_to_db(&candidate);
        let in_library: bool = conn
            .query_row(
                "SELECT 1 FROM audio_file WHERE path = ?1",
                params![cp],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if in_library {
            continue;
        }
        let Ok((size, _)) = stat(&candidate) else { continue };
        let mut hash = None;
        for m in missing.iter().filter(|m| !matched.contains(&m.id)) {
            let same_name = Path::new(&m.path).file_name() == candidate.file_name();
            if m.size == size {
                let h = match &hash {
                    Some(h) => h,
                    None => hash.insert(content_hash(&candidate)?),
                };
                if *h == m.hash {
                    matched.insert(m.id.clone());
                    proposals.push(RelinkProposal {
                        file_id: m.id.clone(),
                        track_id: m.track_id.clone(),
                        old_path: m.path.clone(),
                        new_path: cp.clone(),
                        method: "content".into(),
                    });
                    break;
                }
            }
            if same_name {
                let dur = tags::read(&candidate).ok().and_then(|t| t.duration_ms);
                if let (Some(a), Some(b)) = (dur, m.duration) {
                    if (a - b).abs() <= 1000 {
                        matched.insert(m.id.clone());
                        proposals.push(RelinkProposal {
                            file_id: m.id.clone(),
                            track_id: m.track_id.clone(),
                            old_path: m.path.clone(),
                            new_path: cp.clone(),
                            method: "name_and_duration".into(),
                        });
                        break;
                    }
                }
            }
        }
    }
    Ok(proposals)
}

// ---------------------------------------------------------------------------
// Files of a track
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileRecord {
    pub id: String,
    pub track_id: String,
    pub path: String,
    pub origin: FileOrigin,
    pub size_bytes: i64,
    pub duration_ms: Option<i64>,
    pub format: Option<String>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
    pub bitrate_kbps: Option<i64>,
    pub availability: Availability,
    pub availability_reason: Option<String>,
    pub is_primary: bool,
    /// How this copy differs from the recording, for example "pitched:+4.0".
    pub variant: Option<String>,
}

const FILE_COLUMNS: &str = "id, track_id, path, origin, size_bytes, duration_ms, format, sample_rate,
    channels, bitrate_kbps, availability, availability_reason, is_primary, variant";

fn file_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: r.get(0)?,
        track_id: r.get(1)?,
        path: r.get(2)?,
        origin: r.get(3)?,
        size_bytes: r.get(4)?,
        duration_ms: r.get(5)?,
        format: r.get(6)?,
        sample_rate: r.get(7)?,
        channels: r.get(8)?,
        bitrate_kbps: r.get(9)?,
        availability: r.get(10)?,
        availability_reason: r.get(11)?,
        is_primary: r.get(12)?,
        variant: r.get(13)?,
    })
}

pub fn files_for_track(conn: &Connection, track_id: &str) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM audio_file WHERE track_id = ?1 ORDER BY is_primary DESC, created_at"
    ))?;
    let rows = stmt
        .query_map(params![track_id], file_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn file(conn: &Connection, file_id: &str) -> Result<FileRecord> {
    conn.query_row(
        &format!("SELECT {FILE_COLUMNS} FROM audio_file WHERE id = ?1"),
        params![file_id],
        file_from_row,
    )
    .optional()?
    .ok_or_else(|| Error::NotFound(format!("file {file_id}")))
}

/// The file to play for a track: the primary copy if available, otherwise
/// any available copy.
pub fn playable_file(conn: &Connection, track_id: &str) -> Result<Option<FileRecord>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {FILE_COLUMNS} FROM audio_file
                 WHERE track_id = ?1 AND availability = 'available'
                 ORDER BY is_primary DESC, created_at LIMIT 1"
            ),
            params![track_id],
            file_from_row,
        )
        .optional()?)
}

pub fn set_primary_file(conn: &Connection, file_id: &str) -> Result<()> {
    let f = file(conn, file_id)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE audio_file SET is_primary = (id = ?2) WHERE track_id = ?1",
        params![f.track_id, file_id],
    )?;
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Import job
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct ImportPayload {
    root_id: String,
}

/// A step to run after a successful scan, for example queueing analysis.
pub type AfterImport = Arc<dyn Fn(&Connection) -> Result<()> + Send + Sync>;

pub struct ImportHandler {
    pub probe: Arc<dyn AudioProbe>,
    pub after: Option<AfterImport>,
}

impl Handler for ImportHandler {
    fn kind(&self) -> &'static str {
        kinds::IMPORT
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let payload: ImportPayload = ctx.payload()?;
        let conn = ctx.conn;
        let result = {
            let ctx_ref = &mut *ctx;
            import_root(conn, &payload.root_id, self.probe.as_ref(), |summary| {
                ctx_ref.heartbeat()?;
                ctx_ref.save_checkpoint(summary)
            })
        };
        match result {
            Ok(summary) => {
                tracing::info!(?summary, "import finished");
                if let Some(after) = &self.after {
                    if let Err(e) = after(conn) {
                        tracing::warn!("after-import step failed: {e}");
                    }
                }
                ctx.save_checkpoint(&summary)
            }
            Err(ImportError::Stopped(e)) => Err(e),
            Err(ImportError::Failed(Error::Invalid(m))) | Err(ImportError::Failed(Error::NotFound(m))) => {
                Err(JobError::Fatal(m))
            }
            Err(ImportError::Failed(e)) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests;
