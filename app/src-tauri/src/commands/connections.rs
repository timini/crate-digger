use cd_connectors::config::Connections;
use cd_connectors::credentials::Credential;
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
    settings::set(&*state.db()?, CONFIG_KEY, &config).map_err(err)
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
        .map_err(err)?
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
