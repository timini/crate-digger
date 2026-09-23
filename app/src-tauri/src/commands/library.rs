use std::path::PathBuf;

use cd_core::domain::Field;
use cd_core::library::duplicates::{self, DuplicatePair};
use cd_core::library::search::{self, LibraryPage, LibraryQuery};
use cd_core::library::{self, FileRecord, ImportSummary, LibraryRoot, RelinkProposal};
use cd_core::meta::{self, FieldProvenance, TrackMeta};
use serde::Serialize;
use tauri::State;

use super::{blocking, err, CmdResult};
use crate::probe::SymphoniaProbe;
use crate::state::AppState;

#[tauri::command]
pub fn library_roots(state: State<'_, AppState>) -> CmdResult<Vec<LibraryRoot>> {
    library::roots(&*state.db()?).map_err(err)
}

/// Add a music folder and start indexing it in place.
#[tauri::command]
pub fn library_add_root(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let conn = state.db()?;
    let id = library::add_root(&conn, &PathBuf::from(path)).map_err(err)?;
    library::enqueue_import(&conn, &id).map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(id)
}

#[tauri::command]
pub fn library_remove_root(state: State<'_, AppState>, root_id: String) -> CmdResult<()> {
    library::remove_root(&*state.db()?, &root_id).map_err(err)
}

#[tauri::command]
pub fn library_rescan(state: State<'_, AppState>) -> CmdResult<usize> {
    let conn = state.db()?;
    let roots = library::roots(&conn).map_err(err)?;
    for r in &roots {
        library::enqueue_import(&conn, &r.id).map_err(err)?;
    }
    drop(conn);
    state.notify_workers();
    Ok(roots.len())
}

#[tauri::command]
pub fn library_search(state: State<'_, AppState>, query: LibraryQuery) -> CmdResult<LibraryPage> {
    search::search(&*state.db()?, &query).map_err(err)
}

#[derive(Serialize)]
pub struct TrackDetail {
    track_id: String,
    meta: TrackMeta,
    provenance: Vec<FieldProvenance>,
    files: Vec<FileRecord>,
    rating: Option<String>,
    kept: bool,
    playlists: Vec<(String, String)>,
}

#[tauri::command]
pub fn track_detail(state: State<'_, AppState>, track_id: String) -> CmdResult<TrackDetail> {
    let conn = state.db()?;
    let track_id = duplicates::resolve_track_id(&conn, &track_id).map_err(err)?;
    let rating = conn
        .query_row(
            "SELECT kind FROM effective_rating WHERE track_id = ?1",
            [&track_id],
            |r| r.get(0),
        )
        .ok();
    let kept = conn
        .query_row(
            "SELECT 1 FROM keep_decision WHERE track_id = ?1",
            [&track_id],
            |_| Ok(()),
        )
        .is_ok();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT p.id, p.name FROM playlist p JOIN playlist_entry e ON e.playlist_id = p.id
             WHERE e.track_id = ?1 ORDER BY p.name",
        )
        .map_err(err)?;
    let playlists = stmt
        .query_map([&track_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    Ok(TrackDetail {
        meta: meta::effective(&conn, &track_id).map_err(err)?,
        provenance: meta::provenance(&conn, &track_id).map_err(err)?,
        files: library::files_for_track(&conn, &track_id).map_err(err)?,
        rating,
        kept,
        playlists,
        track_id,
    })
}

/// Correct a field. `None` removes the correction so the file's own value
/// applies again.
#[tauri::command]
pub fn track_set_field(
    state: State<'_, AppState>,
    track_id: String,
    field: Field,
    value: Option<String>,
) -> CmdResult<()> {
    meta::set_correction(&*state.db()?, &track_id, field, value.as_deref()).map_err(err)
}

#[tauri::command]
pub fn file_set_primary(state: State<'_, AppState>, file_id: String) -> CmdResult<()> {
    library::set_primary_file(&*state.db()?, &file_id).map_err(err)
}

#[tauri::command]
pub async fn relink_find(state: State<'_, AppState>, folder: String) -> CmdResult<Vec<RelinkProposal>> {
    blocking(&state, move |conn| {
        library::find_moved(conn, &PathBuf::from(folder), &SymphoniaProbe)
    })
    .await
}

#[tauri::command]
pub async fn relink_apply(state: State<'_, AppState>, file_id: String, path: String) -> CmdResult<()> {
    blocking(&state, move |conn| {
        library::relink_file(conn, &file_id, &PathBuf::from(path), &SymphoniaProbe)
    })
    .await
}

#[tauri::command]
pub async fn library_check_files(state: State<'_, AppState>) -> CmdResult<ImportSummary> {
    blocking(&state, |conn| library::check_files(conn, &SymphoniaProbe)).await
}

#[tauri::command]
pub fn duplicates_list(state: State<'_, AppState>) -> CmdResult<Vec<DuplicatePair>> {
    duplicates::suggestions(&*state.db()?, 200).map_err(err)
}

#[tauri::command]
pub fn duplicates_merge(state: State<'_, AppState>, keep: String, remove: String) -> CmdResult<()> {
    duplicates::merge(
        &*state.db()?,
        &keep,
        &remove,
        "Confirmed as the same recording by the user in the library duplicates view",
    )
    .map_err(err)
}

#[tauri::command]
pub fn duplicates_dismiss(state: State<'_, AppState>, a: String, b: String) -> CmdResult<()> {
    duplicates::dismiss(&*state.db()?, &a, &b).map_err(err)
}
