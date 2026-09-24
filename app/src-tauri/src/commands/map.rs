//! The library map: read the saved map, and rebuild it in the background.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cd_core::analysis::handler::Analyzer;
use cd_core::similarity::{
    self, compute::Control, Coverage, MapInfo, MapNeighbour, MapPoint, Params, Unplaced,
};
use serde::Serialize;
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

#[derive(Default)]
pub struct MapBuild {
    cancel: AtomicBool,
    progress: AtomicU32,
}

#[derive(Serialize)]
pub struct Building {
    progress: u32,
}

#[derive(Serialize)]
pub struct MapView {
    map: Option<MapInfo>,
    coverage: Coverage,
    points: Vec<MapPoint>,
    building: Option<Building>,
    /// Why the last rebuild failed, if it did.
    last_error: Option<String>,
}

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[tauri::command]
pub fn map_get(state: State<'_, AppState>) -> CmdResult<MapView> {
    let conn = state.db()?;
    let map = similarity::current(&conn).map_err(err)?;
    let points = match &map {
        Some(m) => similarity::points(&conn, &m.id).map_err(err)?,
        None => vec![],
    };
    Ok(MapView {
        coverage: similarity::coverage(&conn, &state.analyzer.version()).map_err(err)?,
        map,
        points,
        building: state.map_build.lock().unwrap().as_ref().map(|b| Building {
            progress: b.progress.load(Ordering::Relaxed),
        }),
        last_error: LAST_ERROR.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub fn map_edges(state: State<'_, AppState>, neighbours: usize) -> CmdResult<Vec<(String, String, f32)>> {
    let conn = state.db()?;
    match similarity::current(&conn).map_err(err)? {
        Some(m) => similarity::edges(&conn, &m.id, neighbours).map_err(err),
        None => Ok(vec![]),
    }
}

#[tauri::command]
pub fn map_neighbours(state: State<'_, AppState>, track_id: String) -> CmdResult<Vec<MapNeighbour>> {
    let conn = state.db()?;
    match similarity::current(&conn).map_err(err)? {
        Some(m) => similarity::neighbours(&conn, &m.id, &track_id).map_err(err),
        None => Ok(vec![]),
    }
}

#[tauri::command]
pub fn map_unplaced(state: State<'_, AppState>) -> CmdResult<Vec<Unplaced>> {
    similarity::unplaced(&*state.db()?, &state.analyzer.version()).map_err(err)
}

/// Start a rebuild with the current analysis model. The saved map stays
/// until the new one is complete.
#[tauri::command]
pub fn map_rebuild(state: State<'_, AppState>, params: Params) -> CmdResult<()> {
    params.validate()?;
    let build = {
        let mut slot = state.map_build.lock().unwrap();
        if slot.is_some() {
            return Err("The map is already being rebuilt.".into());
        }
        let b = Arc::new(MapBuild::default());
        *slot = Some(b.clone());
        b
    };
    *LAST_ERROR.lock().unwrap() = None;
    let version = state.analyzer.version();
    let path = state.db_path.clone();
    let app_slot = Arc::clone(&build);
    let finish = move |result: Result<(), String>| {
        if let Err(e) = result {
            tracing::warn!("map rebuild: {e}");
            *LAST_ERROR.lock().unwrap() = Some(e);
        }
    };
    let state_slot = state.map_build.clone();
    std::thread::Builder::new()
        .name("map-rebuild".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                // Read with a separate connection so playback and the UI are not blocked.
                let conn = cd_core::db::open_existing(&path).map_err(err)?;
                let inputs = similarity::load(&conn, &version).map_err(err)?;
                let ctl = Control {
                    cancel: &app_slot.cancel,
                    progress: &app_slot.progress,
                };
                let built = similarity::compute(&inputs, &params, &ctl)
                    .map_err(|_| "The rebuild was cancelled; the previous map is kept.".to_string())?;
                similarity::save(&conn, &inputs, &built, cd_core::util::now_ms()).map_err(err)?;
                Ok(())
            })();
            finish(result);
            *state_slot.lock().unwrap() = None;
        })
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn map_cancel(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(b) = &*state.map_build.lock().unwrap() {
        b.cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}
