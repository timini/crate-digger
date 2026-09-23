use cd_core::domain::RatingKind;
use cd_core::review::{self, QueueStats, ReviewCard, Undone};
use cd_core::util::now_ms;
use cd_core::{discovery, settings};
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;
use crate::workers::DEMO_CONNECTOR;

#[tauri::command]
pub fn review_next(state: State<'_, AppState>, limit: i64) -> CmdResult<Vec<ReviewCard>> {
    review::next(&*state.db()?, &state.session_id, limit).map_err(err)
}

#[tauri::command]
pub fn review_stats(state: State<'_, AppState>) -> CmdResult<QueueStats> {
    review::stats(&*state.db()?, &state.session_id).map_err(err)
}

fn after_preference_change(state: &AppState) {
    // The rating is already on disk; reranking runs separately and never
    // touches the player.
    let version = {
        use cd_core::analysis::handler::Analyzer;
        state.analyzer.version()
    };
    state.spawn_background("rerank", move |conn| {
        if let Err(e) = review::rerank(conn, &version) {
            tracing::warn!("rerank failed: {e}");
        }
    });
}

#[tauri::command]
pub fn review_rate(state: State<'_, AppState>, track_id: String, kind: RatingKind) -> CmdResult<String> {
    let id = review::rate(&*state.db()?, &track_id, kind, &state.session_id, now_ms()).map_err(err)?;
    after_preference_change(&state);
    Ok(id)
}

#[tauri::command]
pub fn review_skip(state: State<'_, AppState>, track_id: String) -> CmdResult<String> {
    review::skip(&*state.db()?, &track_id, &state.session_id, now_ms()).map_err(err)
}

#[tauri::command]
pub fn review_undo(state: State<'_, AppState>) -> CmdResult<Option<Undone>> {
    let undone = review::undo(&*state.db()?, &state.session_id, now_ms()).map_err(err)?;
    if undone.is_some() {
        after_preference_change(&state);
    }
    Ok(undone)
}

#[tauri::command]
pub fn review_keep(state: State<'_, AppState>, track_id: String, keep: bool) -> CmdResult<()> {
    let conn = state.db()?;
    if keep {
        // Records the decision and queues the move into the archive.
        cd_core::archive::keep_track(&conn, &track_id, now_ms()).map_err(err)?;
        drop(conn);
        state.notify_workers();
        Ok(())
    } else {
        // Audio already in the archive stays there; only the decision changes.
        review::unkeep(&conn, &track_id).map_err(err)
    }
}

#[derive(serde::Serialize)]
pub struct StorageStatus {
    staging_dir: String,
    archive_dir: String,
    staging_used_bytes: u64,
    staging_budget_bytes: u64,
}

#[tauri::command]
pub fn storage_status(state: State<'_, AppState>) -> CmdResult<StorageStatus> {
    let status = state.scheduler.status(&*state.db()?).map_err(err)?;
    Ok(StorageStatus {
        staging_dir: state.staging_dir().display().to_string(),
        archive_dir: state.archive_dir().display().to_string(),
        staging_used_bytes: status.staging_used_bytes,
        staging_budget_bytes: status.staging_budget_bytes,
    })
}

/// Delete temporary audio that is not kept or in a playlist. Unreviewed
/// tracks are only included when the user asks.
#[tauri::command]
pub async fn staging_clear(
    state: State<'_, AppState>,
    include_unreviewed: bool,
) -> CmdResult<cd_core::archive::ClearSummary> {
    let summary = super::blocking(&state, move |conn| {
        cd_core::archive::clear_temporary(conn, include_unreviewed)
    })
    .await?;
    state.notify_workers();
    Ok(summary)
}

/// Ask for more candidates: demo discovery when it is on, otherwise the
/// live sources.
#[tauri::command]
pub fn review_find_more(state: State<'_, AppState>) -> CmdResult<()> {
    let conn = state.db()?;
    let connector = if settings::get_or(&conn, settings::keys::DEMO_DISCOVERY, false).map_err(err)? {
        DEMO_CONNECTOR
    } else {
        cd_connectors::discovery::LIVE_SOURCE
    };
    discovery::request_discovery(&conn, connector, 20, now_ms()).map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(())
}

#[tauri::command]
pub fn demo_discovery_get(state: State<'_, AppState>) -> CmdResult<bool> {
    settings::get_or(&*state.db()?, settings::keys::DEMO_DISCOVERY, false).map_err(err)
}

#[tauri::command]
pub fn demo_discovery_set(state: State<'_, AppState>, enabled: bool) -> CmdResult<()> {
    settings::set(&*state.db()?, settings::keys::DEMO_DISCOVERY, &enabled).map_err(err)
}
