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
    state.spawn_background("rerank", |conn| {
        if let Err(e) = review::rerank(conn) {
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
        review::keep(&conn, &track_id, now_ms()).map_err(err)
    } else {
        review::unkeep(&conn, &track_id).map_err(err)
    }
}

/// Ask for more candidates. Until real sources exist (#11) this only works
/// with demo discovery turned on.
#[tauri::command]
pub fn review_find_more(state: State<'_, AppState>) -> CmdResult<()> {
    let conn = state.db()?;
    if !settings::get_or(&conn, settings::keys::DEMO_DISCOVERY, false).map_err(err)? {
        return Err(
            "No discovery sources are connected yet. Discogs, tracklists and Soulseek arrive in a later \
             release; turn on demo discovery in Settings to try reviewing with generated tones."
                .into(),
        );
    }
    discovery::request_discovery(&conn, DEMO_CONNECTOR, 10, now_ms()).map_err(err)?;
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
