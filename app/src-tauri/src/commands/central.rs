//! Catalogue sign-in, sharing and private backups.

use std::time::Duration;

use cd_core::{backup, settings, sharing};
use serde::Serialize;
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

const EMAIL_KEY: &str = "google_email";

#[derive(Serialize)]
pub struct CentralStatus {
    /// Endpoint and client id are set.
    configured: bool,
    signed_in: bool,
    email: Option<String>,
    sharing: bool,
    outbox: sharing::OutboxSummary,
}

#[tauri::command]
pub fn central_status(state: State<'_, AppState>) -> CmdResult<CentralStatus> {
    let conn = state.db()?;
    let client = state.central.client();
    Ok(CentralStatus {
        configured: client.is_some(),
        signed_in: client
            .map(|c| c.auth.signed_in().unwrap_or(false))
            .unwrap_or(false),
        email: settings::get(&conn, EMAIL_KEY).map_err(err)?,
        sharing: sharing::enabled(&conn).map_err(err)?,
        outbox: sharing::outbox(&conn).map_err(err)?,
    })
}

/// Opens Google's sign-in page in the browser and waits up to five minutes.
#[tauri::command]
pub async fn central_sign_in(app: tauri::AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    use tauri_plugin_opener::OpenerExt;
    let client = state
        .central
        .client()
        .ok_or("Add the catalogue's address and Google client id in Settings first.")?;
    let pending = client.auth.begin().map_err(err)?;
    app.opener()
        .open_url(pending.url.clone(), None::<&str>)
        .map_err(err)?;
    let auth = client.auth.clone();
    let email = tauri::async_runtime::spawn_blocking(move || auth.finish(pending, Duration::from_secs(300)))
        .await
        .map_err(err)?
        .map_err(err)?;
    let conn = state.db()?;
    settings::set(&conn, EMAIL_KEY, &email).map_err(err)?;
    // Anything paused for sign-in can go now.
    let _ = cd_core::jobs::set_connector_status(
        &conn,
        sharing::CONNECTOR,
        cd_core::jobs::ConnectorStatus::Ok,
        None,
        cd_core::util::now_ms(),
    );
    drop(conn);
    state.notify_workers();
    Ok(email)
}

#[tauri::command]
pub async fn central_sign_out(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(client) = state.central.client() {
        tauri::async_runtime::spawn_blocking(move || client.auth.sign_out())
            .await
            .map_err(err)?
            .map_err(err)?;
    }
    settings::set(&*state.db()?, EMAIL_KEY, &None::<String>).map_err(err)
}

/// Turn contributing on or off. On queues what is already identified.
#[tauri::command]
pub fn sharing_set(state: State<'_, AppState>, enabled: bool) -> CmdResult<usize> {
    let conn = state.db()?;
    settings::set(&conn, settings::keys::SHARING, &enabled).map_err(err)?;
    if !enabled {
        return Ok(0);
    }
    let tracks: Vec<String> = conn
        .prepare("SELECT DISTINCT track_id FROM track_external_id")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .map_err(err)?;
    let now = cd_core::util::now_ms();
    let mut queued = 0;
    for t in tracks {
        if sharing::queue_share(&conn, &t, now).map_err(err)? {
            queued += 1;
        }
    }
    drop(conn);
    state.notify_workers();
    Ok(queued)
}

fn client(state: &AppState) -> CmdResult<std::sync::Arc<cd_connectors::central::Central>> {
    state
        .central
        .client()
        .ok_or_else(|| "Add the catalogue's address and Google client id in Settings first.".into())
}

#[tauri::command]
pub async fn backup_now(state: State<'_, AppState>) -> CmdResult<cd_protocol::backup::BackupInfo> {
    let snapshot = backup::snapshot(&*state.db()?, cd_core::util::now_ms()).map_err(err)?;
    let client = client(&state)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    tauri::async_runtime::spawn_blocking(move || client.put_backup(&id, &snapshot))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
pub async fn backups_list(state: State<'_, AppState>) -> CmdResult<Vec<cd_protocol::backup::BackupInfo>> {
    let client = client(&state)?;
    tauri::async_runtime::spawn_blocking(move || client.list_backups())
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
pub async fn backup_restore(state: State<'_, AppState>, id: String) -> CmdResult<backup::RestoreSummary> {
    let client = client(&state)?;
    let snapshot = tauri::async_runtime::spawn_blocking(move || client.get_backup(&id))
        .await
        .map_err(err)?
        .map_err(err)?;
    let summary = backup::restore(&*state.db()?, &snapshot, cd_core::util::now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(summary)
}

#[tauri::command]
pub async fn backup_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let client = client(&state)?;
    tauri::async_runtime::spawn_blocking(move || client.delete_backup(&id))
        .await
        .map_err(err)?
        .map_err(err)
}
