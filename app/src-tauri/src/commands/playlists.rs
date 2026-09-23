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
