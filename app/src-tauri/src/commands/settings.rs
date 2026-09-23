use std::path::PathBuf;

use cd_core::settings::{self, keys, UserLimits};
use serde::Serialize;
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct AppSettings {
    archive_dir: String,
    archive_dir_is_default: bool,
    staging_dir: String,
    limits: UserLimits,
    demo_discovery: bool,
    close_to_tray: bool,
    onboarded: bool,
    data_dir: String,
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    let archive_dir = state.archive_dir();
    let staging_dir = state.staging_dir();
    let close_to_tray = state.close_to_tray();
    let conn = state.db()?;
    Ok(AppSettings {
        archive_dir_is_default: archive_dir == state.default_archive_dir,
        archive_dir: archive_dir.display().to_string(),
        staging_dir: staging_dir.display().to_string(),
        limits: settings::limits(&conn).map_err(err)?,
        demo_discovery: settings::get_or(&conn, keys::DEMO_DISCOVERY, false).map_err(err)?,
        close_to_tray,
        onboarded: settings::get_or(&conn, keys::ONBOARDED, false).map_err(err)?,
        data_dir: state.data_dir.display().to_string(),
    })
}

fn check_folder(path: &str) -> CmdResult<()> {
    let p = PathBuf::from(path);
    std::fs::create_dir_all(&p).map_err(|e| format!("Cannot use {path}: {e}"))?;
    // Make sure we can write there before accepting it.
    let probe = p.join(".crate-digger-write-test");
    std::fs::write(&probe, b"ok").map_err(|e| format!("Cannot write to {path}: {e}"))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

/// Where kept tracks are moved. Files already archived stay where they are.
#[tauri::command]
pub fn settings_set_archive_dir(state: State<'_, AppState>, path: Option<String>) -> CmdResult<()> {
    let conn = state.db()?;
    match path {
        Some(p) => {
            check_folder(&p)?;
            settings::set(&conn, keys::ARCHIVE_DIR, &p).map_err(err)
        }
        None => conn
            .execute("DELETE FROM setting WHERE key = ?1", [keys::ARCHIVE_DIR])
            .map(|_| ())
            .map_err(err),
    }
}

/// Where downloads wait. Takes effect for new downloads after a restart.
#[tauri::command]
pub fn settings_set_staging_dir(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    check_folder(&path)?;
    settings::set(&*state.db()?, keys::STAGING_DIR, &path).map_err(err)
}

#[tauri::command]
pub fn settings_set_limits(state: State<'_, AppState>, limits: UserLimits) -> CmdResult<()> {
    limits.validate()?;
    settings::set(&*state.db()?, keys::LIMITS, &limits).map_err(err)?;
    state.scheduler.set_limits(limits.scheduler_limits());
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn settings_set_close_to_tray(state: State<'_, AppState>, enabled: bool) -> CmdResult<()> {
    settings::set(&*state.db()?, keys::CLOSE_TO_TRAY, &enabled).map_err(err)
}

#[tauri::command]
pub fn onboarding_complete(state: State<'_, AppState>) -> CmdResult<()> {
    settings::set(&*state.db()?, keys::ONBOARDED, &true).map_err(err)
}
