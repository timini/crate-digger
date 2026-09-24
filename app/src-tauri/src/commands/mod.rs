//! Tauri commands. Each returns a plain string error the UI can show.

pub mod analysis;
pub mod central;
pub mod connections;
pub mod identity;
pub mod jobs;
pub mod library;
pub mod player;
pub mod playlists;
pub mod review;
pub mod settings;

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

pub type CmdResult<T> = Result<T, String>;

pub fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run slow work (folder walks, decoding) off the UI thread on its own
/// database connection, so it never holds the shared connection's lock.
pub async fn blocking<T, F>(state: &AppState, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> cd_core::Result<T> + Send + 'static,
{
    let path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = cd_core::db::open_existing(&path).map_err(err)?;
        f(&conn).map_err(err)
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    schema_version: i64,
    data_dir: String,
    db_path: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> CmdResult<AppInfo> {
    let conn = state.db()?;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        schema_version: cd_core::db::schema_version(&conn).map_err(err)?,
        data_dir: state.data_dir.display().to_string(),
        db_path: state.db_path.display().to_string(),
    })
}
