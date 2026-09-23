use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

pub struct AppState {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub db: Mutex<Connection>,
}

impl AppState {
    pub fn open(data_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("crate-digger.sqlite");
        let conn = cd_core::db::open(&db_path)?;
        tracing::info!(path = %db_path.display(), "opened database");
        Ok(AppState {
            data_dir: data_dir.to_path_buf(),
            db_path,
            db: Mutex::new(conn),
        })
    }
}
