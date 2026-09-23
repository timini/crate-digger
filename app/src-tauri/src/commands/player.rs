use cd_audio::{AudioError, PlayState, PlayerStatus};
use cd_core::library;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::State;

use super::{blocking, err, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct NowPlaying {
    track_id: Option<String>,
    #[serde(flatten)]
    status: Option<PlayerStatus>,
}

/// Load a track's playable file and start it. Missing or unreadable files
/// are recorded on the file record and reported with what to do next.
#[tauri::command]
pub fn player_play_track(
    state: State<'_, AppState>,
    track_id: String,
    start_ms: Option<u64>,
) -> CmdResult<()> {
    let conn = state.db()?;
    let file = library::playable_file(&conn, &track_id).map_err(err)?;
    let Some(file) = file else {
        let reason: Option<String> = conn
            .query_row(
                "SELECT availability_reason FROM audio_file WHERE track_id = ?1
                 ORDER BY is_primary DESC LIMIT 1",
                [&track_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?
            .flatten();
        return Err(reason.unwrap_or_else(|| "This track has no local audio yet.".into()));
    };
    let player = state.player()?;
    match player.load(std::path::Path::new(&file.path), start_ms.unwrap_or(0), true) {
        Ok(_) => {
            *state.now_playing.lock().unwrap() = Some(track_id);
            state.scheduler.set_playback_active(true);
            Ok(())
        }
        Err(e) => {
            match &e {
                AudioError::NotFound(_) => library::mark_missing(&conn, &file.id, &file.path).map_err(err)?,
                AudioError::Corrupt(m) | AudioError::Unsupported(m) => {
                    library::mark_corrupt(&conn, &file.id, m).map_err(err)?
                }
                AudioError::Unreadable { .. } => {}
            }
            Err(e.user_message())
        }
    }
}

#[tauri::command]
pub fn player_toggle(state: State<'_, AppState>) -> CmdResult<()> {
    let p = state.player()?;
    p.toggle();
    state.scheduler.set_playback_active(p.is_playing());
    Ok(())
}

#[tauri::command]
pub fn player_pause(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(p) = state.player_if_started() {
        p.pause();
    }
    state.scheduler.set_playback_active(false);
    Ok(())
}

#[tauri::command]
pub fn player_seek(state: State<'_, AppState>, ms: u64) -> CmdResult<()> {
    state.player()?.seek(ms);
    Ok(())
}

#[tauri::command]
pub fn player_set_volume(state: State<'_, AppState>, volume: f32) -> CmdResult<()> {
    state.player()?.set_volume(volume);
    Ok(())
}

#[tauri::command]
pub fn player_status(state: State<'_, AppState>) -> CmdResult<NowPlaying> {
    let status = state.player_if_started().map(|p| p.status());
    let playing = status
        .as_ref()
        .map(|s| s.state == PlayState::Playing)
        .unwrap_or(false);
    state.scheduler.set_playback_active(playing);
    Ok(NowPlaying {
        track_id: state.now_playing.lock().unwrap().clone(),
        status,
    })
}

/// Peak overview for a track's playable file, computed once and cached.
#[tauri::command]
pub async fn track_waveform(state: State<'_, AppState>, track_id: String) -> CmdResult<Option<Vec<u8>>> {
    blocking(&state, move |conn| {
        let Some(file) = library::playable_file(conn, &track_id)? else {
            return Ok(None);
        };
        let cached: Option<Vec<u8>> = conn
            .query_row("SELECT waveform FROM audio_file WHERE id = ?1", [&file.id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        if cached.is_some() {
            return Ok(cached);
        }
        match cd_audio::waveform::peaks(std::path::Path::new(&file.path), 800) {
            Ok(peaks) => {
                conn.execute(
                    "UPDATE audio_file SET waveform = ?2 WHERE id = ?1",
                    rusqlite::params![file.id, peaks],
                )?;
                Ok(Some(peaks))
            }
            Err(e) => {
                tracing::warn!("waveform failed for {}: {e}", file.path);
                Ok(None)
            }
        }
    })
    .await
}
