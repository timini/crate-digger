//! Tauri commands. Each returns a plain string error the UI can show.

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

pub type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
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
    let conn = state.db.lock().map_err(err)?;
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        schema_version: cd_core::db::schema_version(&conn).map_err(err)?,
        data_dir: state.data_dir.display().to_string(),
        db_path: state.db_path.display().to_string(),
    })
}
