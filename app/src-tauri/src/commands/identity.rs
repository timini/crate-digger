use cd_core::identity::policy::Relation;
use cd_core::identity::{self, Applied, Conflict};
use tauri::State;

use super::{err, CmdResult};
use crate::state::AppState;

#[tauri::command]
pub fn identity_conflicts(state: State<'_, AppState>) -> CmdResult<Vec<Conflict>> {
    identity::open_conflicts(&*state.db()?).map_err(err)
}

#[tauri::command]
pub fn identity_conflict_count(state: State<'_, AppState>) -> CmdResult<i64> {
    state
        .db()?
        .query_row(
            "SELECT COUNT(*) FROM identity_conflict WHERE state = 'open'",
            [],
            |r| r.get(0),
        )
        .map_err(err)
}

#[tauri::command]
pub fn identity_resolve(
    state: State<'_, AppState>,
    conflict_id: String,
    relation: Relation,
) -> CmdResult<Applied> {
    identity::resolve_conflict(&*state.db()?, &conflict_id, relation).map_err(err)
}
