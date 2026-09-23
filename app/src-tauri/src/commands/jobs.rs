use cd_core::domain::JobState;
use cd_core::jobs::scheduler::SchedulerStatus;
use cd_core::jobs::{self, ConnectorHealth, Job, JobCount};
use cd_core::util::now_ms;
use serde::Serialize;
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct Activity {
    jobs: Vec<Job>,
    counts: Vec<JobCount>,
    scheduler: SchedulerStatus,
    connectors: Vec<ConnectorHealth>,
}

#[tauri::command]
pub fn activity(state: State<'_, AppState>, states: Vec<JobState>, limit: i64) -> CmdResult<Activity> {
    let conn = state.db()?;
    state.scheduler.refresh(&conn).map_err(err)?;
    Ok(Activity {
        jobs: jobs::list(&conn, &states, limit).map_err(err)?,
        counts: jobs::counts(&conn).map_err(err)?,
        scheduler: state.scheduler.status(&conn).map_err(err)?,
        connectors: jobs::connector_health(&conn).map_err(err)?,
    })
}

#[tauri::command]
pub fn jobs_pause_all(state: State<'_, AppState>) -> CmdResult<usize> {
    jobs::pause_all(&*state.db()?, now_ms()).map_err(err)
}

#[tauri::command]
pub fn jobs_resume_all(state: State<'_, AppState>) -> CmdResult<usize> {
    let n = jobs::resume_all(&*state.db()?, now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(n)
}

#[tauri::command]
pub fn job_cancel(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    jobs::cancel(&*state.db()?, &id, now_ms()).map_err(err)
}

#[tauri::command]
pub fn job_retry(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    jobs::retry(&*state.db()?, &id, now_ms()).map_err(err)?;
    state.notify_workers();
    Ok(())
}
