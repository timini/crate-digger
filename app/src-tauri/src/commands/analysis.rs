use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cd_core::analysis::handler::Analyzer;
use cd_core::settings;
use tauri::State;

use super::{err, CmdResult};
use crate::models::{self, ModelStatus};
use crate::state::AppState;

/// The model being downloaded and its byte count.
type Progress = Mutex<Option<(String, Arc<AtomicU64>)>>;

fn progress() -> &'static Progress {
    static P: OnceLock<Progress> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

#[tauri::command]
pub fn models_list(state: State<'_, AppState>) -> CmdResult<Vec<ModelStatus>> {
    Ok(models::statuses(&*state.db()?, &state.data_dir))
}

/// Download a model. Only runs when the user asks.
#[tauri::command]
pub async fn model_download(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let info = cd_analyzer::models::info(&id).ok_or_else(|| format!("unknown model {id}"))?;
    let counter = Arc::new(AtomicU64::new(0));
    *progress().lock().unwrap() = Some((id.clone(), counter.clone()));
    let data_dir = state.data_dir.clone();
    let result = tauri::async_runtime::spawn_blocking(move || models::download(&data_dir, info, counter))
        .await
        .map_err(err)?;
    *progress().lock().unwrap() = None;
    result
}

#[derive(serde::Serialize)]
pub struct DownloadProgress {
    id: String,
    bytes: u64,
}

#[tauri::command]
pub fn model_download_progress() -> Option<DownloadProgress> {
    progress()
        .lock()
        .unwrap()
        .as_ref()
        .map(|(id, c)| DownloadProgress {
            id: id.clone(),
            bytes: c.load(Ordering::Relaxed),
        })
}

/// Choose the model whose embeddings drive ranking (None for the built-in
/// baseline), and queue re-analysis for tracks that still have audio.
#[tauri::command]
pub fn model_choose(state: State<'_, AppState>, id: Option<String>) -> CmdResult<()> {
    let conn = state.db()?;
    match &id {
        Some(id) => {
            let info = cd_analyzer::models::info(id).ok_or_else(|| format!("unknown model {id}"))?;
            if !models::installed(&state.data_dir, info) {
                return Err("Download the model before choosing it.".into());
            }
            settings::set(&conn, models::SETTING, id).map_err(err)?;
        }
        None => {
            conn.execute("DELETE FROM setting WHERE key = ?1", [models::SETTING])
                .map_err(err)?;
        }
    }
    let chosen = models::chosen(&conn, &state.data_dir);
    state.analyzer.set(Arc::new(crate::workers::analyzer(chosen)));
    crate::workers::plan_analysis(&conn, &state.analyzer.version()).map_err(err)?;
    drop(conn);
    state.notify_workers();
    Ok(())
}
