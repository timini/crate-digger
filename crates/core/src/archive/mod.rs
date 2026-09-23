//! Staging and the managed archive.
//!
//! Downloads wait in staging. Keeping a track moves its validated audio into
//! the archive at `Artist/Release/NN - Title (Mix).ext`. Every move is
//! journalled in `archive_op` and only ever uses no-replace file operations,
//! so an interrupted move can be finished or undone on the next start
//! without overwriting or losing audio. The source is removed only after
//! the new copy is verified and the database points at it.

pub mod fsops;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::domain::{Availability, CandidateStatus, FileOrigin};
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::library;
use crate::meta::{self, TrackMeta};
use crate::util::{new_id, now_ms};
use crate::{pipeline, playlists, Error, Result};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Byte limits leave room under Windows' 260-character path limit for a
/// typical archive root.
const MAX_DIR_BYTES: usize = 100;
const MAX_STEM_BYTES: usize = 150;

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9", "LPT¹", "LPT²", "LPT³",
];

fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn trim_end_dots_spaces(s: &str) -> &str {
    s.trim_end_matches(['.', ' '])
}

/// Make one path component safe on Windows, macOS and Linux: NFC
/// normalised, no reserved characters or names, no leading dot, no
/// trailing dot or space, and at most `max_bytes` long.
pub fn sanitize_component(input: &str, max_bytes: usize) -> String {
    let normalised: String = input.nfc().collect();
    let replaced: String = normalised
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = trim_end_dots_spaces(collapsed.trim()).to_string();
    if s.starts_with('.') {
        s.insert(0, '_');
    }
    if s.is_empty() {
        s.push('_');
    }
    let stem_upper = s.split('.').next().unwrap_or("").trim_end().to_uppercase();
    if WINDOWS_RESERVED.contains(&stem_upper.as_str()) {
        let dot = s.find('.').unwrap_or(s.len());
        s.insert(dot, '_');
    }
    let t = trim_end_dots_spaces(truncate_bytes(&s, max_bytes)).to_string();
    if t.is_empty() {
        "_".into()
    } else {
        t
    }
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Leading digits of a track number ("3/12" becomes 03); vinyl positions
/// such as "A1" are kept as they are.
fn track_number(v: &Option<String>) -> Option<String> {
    let raw = nonempty(v)?;
    let first = raw.split('/').next().unwrap_or(raw).trim();
    match first.parse::<u32>() {
        Ok(0) => None,
        Ok(n) => Some(format!("{n:02}")),
        Err(_) if !first.is_empty() => Some(first.to_string()),
        Err(_) => None,
    }
}

/// `Artist/Release/NN - Title (Mix).ext`, with `Singles` for a missing
/// release and no number or mix part when those are unknown.
pub fn relative_path(m: &TrackMeta, ext: &str) -> PathBuf {
    let artist = sanitize_component(nonempty(&m.artist).unwrap_or("Unknown Artist"), MAX_DIR_BYTES);
    let release = sanitize_component(nonempty(&m.release).unwrap_or("Singles"), MAX_DIR_BYTES);
    let title = nonempty(&m.title).unwrap_or("Untitled");
    let mut stem = String::new();
    if let Some(n) = track_number(&m.track_number) {
        stem.push_str(&n);
        stem.push_str(" - ");
    }
    stem.push_str(title);
    if let Some(mix) = nonempty(&m.mix) {
        // Avoid "Title (Dub) (Dub)" when the title already names the mix.
        if !title
            .to_lowercase()
            .ends_with(&format!("({})", mix.to_lowercase()))
        {
            stem.push_str(&format!(" ({mix})"));
        }
    }
    let ext = sanitize_component(&ext.to_lowercase(), 10);
    let file = format!("{}.{ext}", sanitize_component(&stem, MAX_STEM_BYTES));
    PathBuf::from(artist).join(release).join(file)
}

fn with_counter(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({n})"),
    };
    path.with_file_name(name)
}

/// First free destination: nothing on disk (including a partial copy), no
/// library file and no unfinished move already using the name.
fn unique_destination(conn: &Connection, planned: &Path) -> Result<PathBuf> {
    for n in 1..10_000u32 {
        let candidate = if n == 1 {
            planned.to_path_buf()
        } else {
            with_counter(planned, n)
        };
        let p = library::path_to_db(&candidate);
        let taken_on_disk = candidate.exists() || fsops::part_path(&candidate).exists();
        let taken_in_db: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM audio_file WHERE path = ?1)
                 OR EXISTS (SELECT 1 FROM archive_op WHERE dest_path = ?1 AND step NOT IN ('done', 'rolled_back'))",
            params![p],
            |r| r.get(0),
        )?;
        if !taken_on_disk && !taken_in_db {
            return Ok(candidate);
        }
    }
    Err(Error::Conflict(format!(
        "no free file name near {}",
        planned.display()
    )))
}

// ---------------------------------------------------------------------------
// Promotion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub root: PathBuf,
    /// Always copy instead of renaming (tests the cross-volume path).
    pub force_copy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Intent,
    Copied,
    DbUpdated,
    Done,
    RolledBack,
}

impl Step {
    fn parse(s: &str) -> Step {
        match s {
            "intent" => Step::Intent,
            "copied" => Step::Copied,
            "db_updated" => Step::DbUpdated,
            "done" => Step::Done,
            _ => Step::RolledBack,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Step::Intent => "intent",
            Step::Copied => "copied",
            Step::DbUpdated => "db_updated",
            Step::Done => "done",
            Step::RolledBack => "rolled_back",
        }
    }
}

struct Op {
    id: String,
    file_id: String,
    src: PathBuf,
    dest: PathBuf,
    step: Step,
}

fn load_op(conn: &Connection, id: &str) -> Result<Op> {
    Ok(conn.query_row(
        "SELECT id, audio_file_id, src_path, dest_path, step FROM archive_op WHERE id = ?1",
        params![id],
        |r| {
            Ok(Op {
                id: r.get(0)?,
                file_id: r.get(1)?,
                src: PathBuf::from(r.get::<_, String>(2)?),
                dest: PathBuf::from(r.get::<_, String>(3)?),
                step: Step::parse(&r.get::<_, String>(4)?),
            })
        },
    )?)
}

fn set_step(conn: &Connection, id: &str, step: Step, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE archive_op SET step = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, step.as_str(), error, now_ms()],
    )?;
    Ok(())
}

fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    Ok(fsops::hash_file(a).map_err(|e| Error::io(a, e))?
        == fsops::hash_file(b).map_err(|e| Error::io(b, e))?)
}

/// Drive an operation forward from whatever step it reached.
fn run_op(conn: &Connection, id: &str, cfg: &ArchiveConfig) -> Result<PathBuf> {
    loop {
        let op = load_op(conn, id)?;
        match op.step {
            Step::Intent => {
                let _ = std::fs::remove_file(fsops::part_path(&op.dest));
                let (src_exists, dest_exists) = (op.src.exists(), op.dest.exists());
                if dest_exists && !src_exists {
                    // The rename finished before the step was recorded.
                    set_step(conn, &op.id, Step::Copied, None)?;
                } else if dest_exists && src_exists {
                    if same_bytes(&op.src, &op.dest)? {
                        set_step(conn, &op.id, Step::Copied, None)?;
                    } else {
                        let msg = format!(
                            "{} appeared while moving; nothing was overwritten and the track stays in staging",
                            op.dest.display()
                        );
                        set_step(conn, &op.id, Step::RolledBack, Some(&msg))?;
                        return Err(Error::Conflict(msg));
                    }
                } else if src_exists {
                    fsops::move_no_replace(&op.src, &op.dest, cfg.force_copy)
                        .map_err(|e| Error::io(&op.dest, e))?;
                    fail::fail_point!("archive.after_move");
                    set_step(conn, &op.id, Step::Copied, None)?;
                } else {
                    let msg = format!("{} is missing; nothing to move", op.src.display());
                    set_step(conn, &op.id, Step::RolledBack, Some(&msg))?;
                    library::mark_missing(conn, &op.file_id, &library::path_to_db(&op.src))?;
                    return Err(Error::Invalid(msg));
                }
            }
            Step::Copied => {
                fail::fail_point!("archive.before_db");
                let tx = conn.unchecked_transaction()?;
                tx.execute(
                    "UPDATE audio_file SET path = ?2, origin = 'archived', library_root_id = NULL,
                            availability = 'available', availability_reason = NULL, last_checked_at = ?3
                     WHERE id = ?1",
                    params![op.file_id, library::path_to_db(&op.dest), now_ms()],
                )?;
                set_step(&tx, &op.id, Step::DbUpdated, None)?;
                tx.commit()?;
                fail::fail_point!("archive.after_db");
            }
            Step::DbUpdated => {
                if op.src.exists() {
                    // Only remove the source when the archive copy is verified.
                    if op.dest.exists() && same_bytes(&op.src, &op.dest)? {
                        std::fs::remove_file(&op.src).map_err(|e| Error::io(&op.src, e))?;
                    } else {
                        tracing::error!(
                            src = %op.src.display(), dest = %op.dest.display(),
                            "archive copy does not match source; keeping both"
                        );
                    }
                }
                if let Some(dir) = op.src.parent() {
                    // Tidy an empty staging folder; ignore failures.
                    let _ = std::fs::remove_dir(dir);
                }
                set_step(conn, &op.id, Step::Done, None)?;
            }
            Step::Done => return Ok(op.dest),
            Step::RolledBack => {
                return Err(Error::Invalid(
                    conn.query_row("SELECT error FROM archive_op WHERE id = ?1", params![id], |r| {
                        r.get::<_, Option<String>>(0)
                    })?
                    .unwrap_or_else(|| "archive move was rolled back".into()),
                ))
            }
        }
    }
}

/// Move a validated staged file into the archive. Idempotent: archived
/// files return their path, and an unfinished earlier attempt is resumed.
/// Imported files stay where they are unless `manage_imported` is set.
pub fn promote(
    conn: &Connection,
    file_id: &str,
    cfg: &ArchiveConfig,
    manage_imported: bool,
) -> Result<PathBuf> {
    let file = library::file(conn, file_id)?;
    match file.origin {
        FileOrigin::Archived => return Ok(PathBuf::from(file.path)),
        FileOrigin::Imported if !manage_imported => {
            return Err(Error::Invalid(
                "imported files stay where they are unless you ask Crate Digger to manage them".into(),
            ))
        }
        _ => {}
    }
    if let Some(op_id) = conn
        .query_row(
            "SELECT id FROM archive_op WHERE audio_file_id = ?1 AND step NOT IN ('done', 'rolled_back')",
            params![file_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return run_op(conn, &op_id, cfg);
    }
    if file.availability != Availability::Available {
        return Err(Error::Invalid(format!(
            "only playable audio can be archived: {}",
            file.availability_reason
                .unwrap_or_else(|| file.availability.to_string())
        )));
    }
    let m = meta::effective(conn, &file.track_id)?;
    let src = PathBuf::from(&file.path);
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    let dest = unique_destination(conn, &cfg.root.join(relative_path(&m, &ext)))?;
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO archive_op (id, audio_file_id, src_path, dest_path, step, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'intent', ?5, ?5)",
        params![id, file_id, file.path, library::path_to_db(&dest), now],
    )?;
    run_op(conn, &id, cfg)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ArchiveRecovery {
    pub finished: usize,
    pub failed: Vec<String>,
}

/// Finish or roll back every interrupted move. Run at startup.
pub fn recover(conn: &Connection, cfg: &ArchiveConfig) -> Result<ArchiveRecovery> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM archive_op WHERE step NOT IN ('done', 'rolled_back')")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        rows
    };
    let mut out = ArchiveRecovery::default();
    for id in ids {
        match run_op(conn, &id, cfg) {
            Ok(_) => out.finished += 1,
            Err(e) => out.failed.push(e.to_string()),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Temporary audio
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClearSummary {
    pub removed_files: usize,
    pub freed_bytes: u64,
    /// Kept or in a playlist: never cleared.
    pub retained: usize,
    /// Not yet reviewed and not included in this clear.
    pub unreviewed_skipped: usize,
}

/// Delete temporary audio the user no longer needs. Tracks that are kept or
/// in a playlist are never touched. Unreviewed tracks are only cleared when
/// `include_unreviewed` is set. Metadata, ratings and analysis features stay.
pub fn clear_temporary(conn: &Connection, include_unreviewed: bool) -> Result<ClearSummary> {
    let files: Vec<(String, String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT id, track_id, path, size_bytes FROM audio_file WHERE origin = 'staged'")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let mut out = ClearSummary::default();
    for (file_id, track_id, path, size) in files {
        if playlists::is_retained(conn, &track_id)? {
            out.retained += 1;
            continue;
        }
        let candidate: Option<(String, String)> = conn
            .query_row(
                "SELECT id, stage FROM candidate WHERE track_id = ?1",
                params![track_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let reviewed = candidate.as_ref().map(|(_, s)| s == "reviewed").unwrap_or(true);
        if !reviewed && !include_unreviewed {
            out.unreviewed_skipped += 1;
            continue;
        }
        let p = Path::new(&path);
        match std::fs::remove_file(p) {
            Ok(()) => out.freed_bytes += size.max(0) as u64,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(p, e)),
        }
        if let Some(dir) = p.parent() {
            let _ = std::fs::remove_dir(dir);
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM archive_op WHERE audio_file_id = ?1",
            params![file_id],
        )?;
        tx.execute("DELETE FROM audio_file WHERE id = ?1", params![file_id])?;
        if let (false, Some((cid, _))) = (reviewed, &candidate) {
            pipeline::set_status(
                &tx,
                cid,
                CandidateStatus::Paused,
                Some("Its temporary audio was cleared. Fetch it again to review it."),
                now_ms(),
            )?;
        }
        tx.commit()?;
        out.removed_files += 1;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Keep and the archive job
// ---------------------------------------------------------------------------

/// Record a keep decision and queue the move of the track's staged audio
/// into the archive.
pub fn keep_track(conn: &Connection, track_id: &str, now: i64) -> Result<Vec<String>> {
    crate::review::keep(conn, track_id, now)?;
    let staged: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM audio_file WHERE track_id = ?1 AND origin = 'staged' AND availability = 'available'",
        )?;
        let rows = stmt
            .query_map(params![track_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        rows
    };
    let mut jobs = Vec::new();
    for file_id in staged {
        jobs.push(
            crate::jobs::enqueue(
                conn,
                &NewJob::new(
                    kinds::ARCHIVE,
                    format!("archive:{file_id}"),
                    serde_json::json!({ "file_id": file_id }),
                ),
                now,
            )?
            .id,
        );
    }
    Ok(jobs)
}

#[derive(Serialize, Deserialize)]
struct ArchivePayload {
    file_id: String,
}

pub struct ArchiveHandler {
    /// Returns the archive root in force right now (it is a setting).
    pub root: Arc<dyn Fn(&Connection) -> PathBuf + Send + Sync>,
}

impl Handler for ArchiveHandler {
    fn kind(&self) -> &'static str {
        kinds::ARCHIVE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let ArchivePayload { file_id } = ctx.payload()?;
        let cfg = ArchiveConfig {
            root: (self.root)(ctx.conn),
            force_copy: false,
        };
        match promote(ctx.conn, &file_id, &cfg, false) {
            Ok(path) => {
                tracing::info!(path = %path.display(), "archived");
                Ok(())
            }
            Err(Error::Invalid(m)) | Err(Error::Conflict(m)) | Err(Error::NotFound(m)) => {
                Err(JobError::Fatal(m))
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests;
