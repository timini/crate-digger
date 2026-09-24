use cd_core::playlists::{self, Playlist, PlaylistEntry};
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

#[tauri::command]
pub fn playlists_list(state: State<'_, AppState>) -> CmdResult<Vec<Playlist>> {
    playlists::list(&*state.db()?).map_err(err)
}

#[tauri::command]
pub fn playlist_create(state: State<'_, AppState>, name: String) -> CmdResult<String> {
    playlists::create(&*state.db()?, &name).map_err(err)
}

#[tauri::command]
pub fn playlist_rename(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    playlists::rename(&*state.db()?, &id, &name).map_err(err)
}

#[tauri::command]
pub fn playlist_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    playlists::delete(&*state.db()?, &id).map_err(err)
}

#[tauri::command]
pub fn playlist_entries(state: State<'_, AppState>, id: String) -> CmdResult<Vec<PlaylistEntry>> {
    playlists::entries(&*state.db()?, &id).map_err(err)
}

#[tauri::command]
pub fn playlist_add(
    state: State<'_, AppState>,
    id: String,
    track_ids: Vec<String>,
    at: Option<usize>,
) -> CmdResult<()> {
    playlists::add_tracks(&*state.db()?, &id, &track_ids, at).map_err(err)
}

#[tauri::command]
pub fn playlist_remove(state: State<'_, AppState>, id: String, position: usize) -> CmdResult<()> {
    playlists::remove_entry(&*state.db()?, &id, position).map_err(err)
}

#[tauri::command]
pub fn playlist_move(state: State<'_, AppState>, id: String, from: usize, to: usize) -> CmdResult<()> {
    playlists::move_entry(&*state.db()?, &id, from, to).map_err(err)
}

/// What an export would contain, and any problems, before writing anything.
#[tauri::command]
pub fn playlist_export_check(state: State<'_, AppState>, id: String) -> CmdResult<cd_core::export::Prepared> {
    cd_core::export::prepare(&*state.db()?, &id).map_err(err)
}

/// Write the playlist as `m3u8` or `rekordbox` to `path`. Returns the number of tracks written.
#[tauri::command]
pub fn playlist_export(
    state: State<'_, AppState>,
    id: String,
    format: String,
    path: String,
) -> CmdResult<usize> {
    let prepared = cd_core::export::prepare(&*state.db()?, &id).map_err(err)?;
    let text = match format.as_str() {
        "m3u8" => cd_core::export::m3u8(&prepared.tracks),
        "rekordbox" => cd_core::export::rekordbox_xml(&prepared.name, &prepared.tracks),
        _ => return Err("Choose M3U8 or Rekordbox XML.".into()),
    };
    std::fs::write(&path, text).map_err(|e| format!("Could not write {path}: {e}"))?;
    Ok(prepared.tracks.len())
}

// Playlist workspaces: brief, seeds, suggestions and fit feedback.

use cd_core::adapters::Seed;
use cd_core::analysis::handler::Analyzer;
use cd_core::review::ReviewCard;
use cd_core::util::now_ms;
use cd_core::workspace::{self, Verdict};

/// The discovery source to use: the demo source when demo discovery is on.
pub fn discovery_connector(conn: &rusqlite::Connection) -> &'static str {
    if cd_core::settings::get_or(conn, cd_core::settings::keys::DEMO_DISCOVERY, false).unwrap_or(false) {
        crate::workers::DEMO_CONNECTOR
    } else {
        cd_connectors::discovery::LIVE_SOURCE
    }
}

#[derive(serde::Serialize)]
pub struct Workspace {
    seeds: Vec<Seed>,
    /// Suggestions waiting for a decision in this playlist.
    ready: usize,
    runs: Vec<cd_core::discovery::SourceRun>,
}

#[tauri::command]
pub fn playlist_workspace(state: State<'_, AppState>, id: String) -> CmdResult<Workspace> {
    let conn = state.db()?;
    let v = state.analyzer.version();
    Ok(Workspace {
        seeds: workspace::seeds(&conn, &id).map_err(err)?,
        ready: workspace::queue(&conn, &id, &v, usize::MAX).map_err(err)?.len(),
        runs: cd_core::discovery::recent_runs(&conn, 200)
            .map_err(err)?
            .into_iter()
            .filter(|r| r.playlist_id.as_deref() == Some(id.as_str()))
            .take(10)
            .collect(),
    })
}

#[tauri::command]
pub fn playlist_ready_counts(
    state: State<'_, AppState>,
) -> CmdResult<std::collections::HashMap<String, usize>> {
    workspace::ready_counts(&*state.db()?, &state.analyzer.version()).map_err(err)
}

#[tauri::command]
pub fn playlist_brief_set(state: State<'_, AppState>, id: String, brief: String) -> CmdResult<()> {
    workspace::set_brief(&*state.db()?, &id, &brief).map_err(err)
}

#[tauri::command]
pub fn playlist_seeds_save(state: State<'_, AppState>, id: String, seeds: Vec<Seed>) -> CmdResult<()> {
    workspace::save_seeds(&*state.db()?, &id, &seeds, now_ms()).map_err(err)
}

/// Turn background discovery for a playlist on or off.
#[tauri::command]
pub fn playlist_discovery_set(state: State<'_, AppState>, id: String, on: bool) -> CmdResult<()> {
    workspace::set_discovery(&*state.db()?, &id, on).map_err(err)
}

/// Look for tracks for this playlist now.
#[tauri::command]
pub fn playlist_discover_now(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let conn = state.db()?;
    workspace::request_discovery(&conn, discovery_connector(&conn), &id, 20, now_ms()).map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn playlist_queue(state: State<'_, AppState>, id: String, limit: usize) -> CmdResult<Vec<ReviewCard>> {
    cd_core::review::next_for_playlist(&*state.db()?, &id, &state.analyzer.version(), limit.min(50))
        .map_err(err)
}

/// Say whether a track fits a playlist. "fits" adds it to the end.
#[tauri::command]
pub fn playlist_feedback(
    state: State<'_, AppState>,
    id: String,
    track_id: String,
    verdict: Verdict,
) -> CmdResult<()> {
    workspace::give_feedback(&*state.db()?, &id, &track_id, verdict, now_ms())
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
pub fn playlist_feedback_undo(state: State<'_, AppState>, id: String) -> CmdResult<Option<String>> {
    workspace::undo_feedback(&*state.db()?, &id, now_ms()).map_err(err)
}
