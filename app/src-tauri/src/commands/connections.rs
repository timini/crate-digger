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
    let external = config.external_slskd;
    *state.connections.write().unwrap() = config;
    // Only one slskd is used: the user's own, or the app's.
    let soulseek = state.soulseek.clone();
    std::thread::spawn(move || {
        if external {
            soulseek.stop();
        } else if !matches!(
            soulseek.status().state,
            cd_connectors::slskd::process::State::Running | cd_connectors::slskd::process::State::Starting
        ) {
            soulseek.start_if_ready();
        }
    });
    resume_live(&conn);
    drop(conn);
    state.notify_workers();
    Ok(())
}

/// A changed connection may fix whatever paused these connectors, so let
/// their jobs try again. They pause again if the problem remains.
fn resume_live(conn: &rusqlite::Connection) {
    for connector in [
        LIVE_SOURCE,
        PAGE_SOURCE,
        "youtube",
        cd_connectors::slskd::CONNECTOR,
        cd_core::metadata_lookup::CONNECTOR,
    ] {
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
    if key == Credential::GoogleRefresh {
        return Err("Sign in with Google instead.".into());
    }
    let secrets = state.secrets.clone();
    tauri::async_runtime::spawn_blocking(move || secrets.set(key, value.as_deref()).map_err(err))
        .await
        .map_err(err)??;
    if matches!(key, Credential::SoulseekUsername | Credential::SoulseekPassword) {
        // A new login takes effect by restarting the app's slskd.
        let soulseek = state.soulseek.clone();
        std::thread::spawn(move || soulseek.start_if_ready());
    }
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
    if service == "slskd" && !state.connections.read().unwrap().external_slskd {
        return managed_soulseek_test(&state);
    }
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

fn managed_soulseek_test(state: &AppState) -> CmdResult<String> {
    use cd_connectors::slskd::process::State as S;
    match state.soulseek.status().state {
        S::Running => Ok("Soulseek is running and signed in.".into()),
        S::SignedOut(why) | S::Failed(why) => Err(why),
        S::Starting => Err("Soulseek is still starting.".into()),
        S::NotInstalled | S::Stopped => Err("Soulseek is not running. Use Set up Soulseek.".into()),
    }
}

#[tauri::command]
pub fn soulseek_status(state: State<'_, AppState>) -> crate::soulseek::Status {
    state.soulseek.status()
}

/// Download slskd on first use (about 60 MB), then start it.
#[tauri::command]
pub async fn soulseek_setup(state: State<'_, AppState>) -> CmdResult<()> {
    let soulseek = state.soulseek.clone();
    tauri::async_runtime::spawn_blocking(move || soulseek.setup())
        .await
        .map_err(err)??;
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn download_choices(state: State<'_, AppState>) -> CmdResult<Vec<cd_core::acquisition::Choice>> {
    cd_core::acquisition::choices(&*state.db()?).map_err(err)
}

#[tauri::command]
pub fn download_choose(state: State<'_, AppState>, candidate_id: String, result_id: String) -> CmdResult<()> {
    cd_core::acquisition::choose(&*state.db()?, &candidate_id, &result_id, cd_core::util::now_ms())
        .map_err(err)?;
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn download_decline(state: State<'_, AppState>, candidate_id: String) -> CmdResult<()> {
    cd_core::acquisition::decline(&*state.db()?, &candidate_id, cd_core::util::now_ms()).map_err(err)
}

#[tauri::command]
pub fn unattended_downloads_get(state: State<'_, AppState>) -> CmdResult<bool> {
    settings::get_or(&*state.db()?, settings::keys::UNATTENDED_DOWNLOADS, false).map_err(err)
}

#[tauri::command]
pub fn unattended_downloads_set(state: State<'_, AppState>, enabled: bool) -> CmdResult<()> {
    settings::set(&*state.db()?, settings::keys::UNATTENDED_DOWNLOADS, &enabled).map_err(err)
}

#[tauri::command]
pub fn metadata_status(
    state: State<'_, AppState>,
    track_id: String,
) -> CmdResult<Option<cd_core::metadata_lookup::LookupStatus>> {
    cd_core::metadata_lookup::status(&*state.db()?, &track_id).map_err(err)
}

/// Look up every analysed track that has not been looked up yet.
#[tauri::command]
pub fn metadata_identify_all(state: State<'_, AppState>) -> CmdResult<usize> {
    let n = cd_core::metadata_lookup::queue_all(&*state.db()?, cd_core::util::now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(n)
}

#[tauri::command]
pub fn metadata_identify(state: State<'_, AppState>, track_id: String) -> CmdResult<()> {
    cd_core::metadata_lookup::queue(&*state.db()?, &track_id, true, cd_core::util::now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn metadata_suggestions(
    state: State<'_, AppState>,
) -> CmdResult<Vec<cd_core::metadata_lookup::Suggestion>> {
    cd_core::metadata_lookup::suggestions(&*state.db()?).map_err(err)
}

#[tauri::command]
pub fn metadata_accept(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    cd_core::metadata_lookup::accept(&*state.db()?, &id, cd_core::util::now_ms()).map_err(err)
}

#[tauri::command]
pub fn metadata_dismiss(state: State<'_, AppState>, track_id: String) -> CmdResult<()> {
    cd_core::metadata_lookup::dismiss(&*state.db()?, &track_id, cd_core::util::now_ms()).map_err(err)
}
