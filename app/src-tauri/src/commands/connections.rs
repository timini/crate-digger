use cd_connectors::config::Connections;
use cd_connectors::credentials::Credential;
use cd_connectors::discovery::{LIVE_SOURCE, PAGE_SOURCE};
use cd_connectors::http::Http;
use cd_core::adapters::Seed;
use cd_core::settings;
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

pub const CONFIG_KEY: &str = "connections";

#[tauri::command]
pub fn connections_get(state: State<'_, AppState>) -> CmdResult<Connections> {
    settings::get_or(&*state.db()?, CONFIG_KEY, Connections::default()).map_err(err)
}
#[tauri::command]
pub fn connections_save(state: State<'_, AppState>, config: Connections) -> CmdResult<()> {
    config.validate().map_err(err)?;
    let conn = state.db()?;
    settings::set(&conn, CONFIG_KEY, &config).map_err(err)?;
    *state.connections.write().unwrap() = config;
    resume_live(&conn);
    drop(conn);
    state.notify_workers();
    Ok(())
}

/// A changed connection may fix whatever paused these connectors, so let
/// their jobs try again. They pause again if the problem remains.
fn resume_live(conn: &rusqlite::Connection) {
    for connector in [LIVE_SOURCE, PAGE_SOURCE, "youtube"] {
        let _ = cd_core::jobs::set_connector_status(
            conn,
            connector,
            cd_core::jobs::ConnectorStatus::Ok,
            None,
            cd_core::util::now_ms(),
        );
    }
}
#[tauri::command]
pub async fn credential_set(
    state: State<'_, AppState>,
    key: Credential,
    value: Option<String>,
) -> CmdResult<()> {
    let secrets = state.secrets.clone();
    tauri::async_runtime::spawn_blocking(move || secrets.set(key, value.as_deref()).map_err(err))
        .await
        .map_err(err)??;
    resume_live(&*state.db()?);
    state.notify_workers();
    Ok(())
}
#[tauri::command]
pub fn discovery_seeds(state: State<'_, AppState>) -> CmdResult<Vec<Seed>> {
    cd_core::discovery::seeds(&*state.db()?).map_err(err)
}
#[tauri::command]
pub fn discovery_seeds_save(state: State<'_, AppState>, seeds: Vec<Seed>) -> CmdResult<()> {
    if seeds.len() > 200
        || seeds
            .iter()
            .any(|s| s.value.trim().is_empty() || s.value.len() > 500)
    {
        return Err("Enter up to 200 seeds, each between 1 and 500 characters.".into());
    }
    let conn = state.db()?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    tx.execute("DELETE FROM seed", []).map_err(err)?;
    for seed in seeds {
        tx.execute(
            "INSERT OR IGNORE INTO seed (id, kind, value, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                cd_core::util::new_id(),
                seed.kind,
                seed.value.trim(),
                cd_core::util::now_ms()
            ],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)
}
#[tauri::command]
pub async fn connection_test(state: State<'_, AppState>, service: String) -> CmdResult<String> {
    let secrets = state.secrets.clone();
    let config = connections_get(state)?;
    tauri::async_runtime::spawn_blocking(move || {
        cd_connectors::probe::probe(&service, &config, &*secrets, std::sync::Arc::new(Http::default()))
            .map_err(err)
    })
    .await
    .map_err(err)?
}

/// Discover from one public page the user supplies.
#[tauri::command]
pub fn discovery_page(state: State<'_, AppState>, url: String) -> CmdResult<()> {
    let url = cd_connectors::discovery::pages::public_url(&url).map_err(err)?;
    let conn = state.db()?;
    cd_core::discovery::request_input(
        &conn,
        PAGE_SOURCE,
        cd_core::discovery::DiscoveryJob::Page { url: url.to_string() },
        50,
        cd_core::util::now_ms(),
    )
    .map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(())
}

/// Discover from text the user pastes, for pages that cannot be fetched.
#[tauri::command]
pub fn discovery_paste(state: State<'_, AppState>, text: String, label: Option<String>) -> CmdResult<()> {
    let conn = state.db()?;
    let now = cd_core::util::now_ms();
    let id = cd_core::discovery::save_supplied_text(&conn, label.as_deref(), &text, now).map_err(err)?;
    cd_core::discovery::request_input(
        &conn,
        PAGE_SOURCE,
        cd_core::discovery::DiscoveryJob::Text { supplied_text_id: id },
        50,
        now,
    )
    .map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn discovery_runs(state: State<'_, AppState>) -> CmdResult<Vec<cd_core::discovery::SourceRun>> {
    cd_core::discovery::recent_runs(&*state.db()?, 10).map_err(err)
}

/// Replace a track's YouTube link with the user's own, after YouTube confirms it exists.
#[tauri::command]
pub async fn youtube_set(state: State<'_, AppState>, track_id: String, url: String) -> CmdResult<()> {
    let yt = crate::workers::youtube(&state);
    let link = tauri::async_runtime::spawn_blocking(move || yt.user_link(&url).map_err(err))
        .await
        .map_err(err)??;
    cd_core::youtube::correct(&*state.db()?, &track_id, &link, cd_core::util::now_ms()).map_err(err)
}

#[tauri::command]
pub fn youtube_prefer(state: State<'_, AppState>, track_id: String, video_id: String) -> CmdResult<()> {
    cd_core::youtube::prefer(&*state.db()?, &track_id, &video_id, cd_core::util::now_ms()).map_err(err)
}

#[tauri::command]
pub fn youtube_reject(state: State<'_, AppState>, track_id: String, video_id: String) -> CmdResult<()> {
    cd_core::youtube::reject(&*state.db()?, &track_id, &video_id, cd_core::util::now_ms()).map_err(err)
}

#[tauri::command]
pub fn youtube_refresh(state: State<'_, AppState>, track_id: String) -> CmdResult<()> {
    cd_core::youtube::queue_lookup(&*state.db()?, &track_id, true, cd_core::util::now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(())
}
