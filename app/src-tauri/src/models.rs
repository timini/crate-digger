//! Downloading and choosing pretrained analysis models.
//!
//! Downloads happen only when the user asks for one. Each file is checked
//! against the checksum pinned in the registry before it is used, and again
//! by the worker every time it loads it.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cd_analyzer::models::{sha256_file, ModelInfo, REGISTRY};
use cd_core::analysis::protocol::ModelRef;
use cd_core::settings;
use serde::Serialize;

pub const SETTING: &str = "analysis_model";

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

pub fn file(data_dir: &Path, m: &ModelInfo) -> PathBuf {
    dir(data_dir).join(format!("{}.onnx", m.id))
}

/// Present with the expected size. The full checksum is verified on
/// download and on every load.
pub fn installed(data_dir: &Path, m: &ModelInfo) -> bool {
    std::fs::metadata(file(data_dir, m))
        .map(|md| md.len() == m.size_bytes)
        .unwrap_or(false)
}

/// The model chosen in Settings, if it is installed.
pub fn chosen(conn: &rusqlite::Connection, data_dir: &Path) -> Option<ModelRef> {
    let id: String = settings::get(conn, SETTING).ok().flatten()?;
    let m = REGISTRY.iter().find(|m| m.id == id)?;
    installed(data_dir, m).then(|| ModelRef {
        id: m.id.into(),
        path: file(data_dir, m).to_string_lossy().into(),
        sha256: m.sha256.into(),
    })
}

#[derive(Serialize)]
pub struct ModelStatus {
    id: &'static str,
    licence: &'static str,
    size_bytes: u64,
    dims: usize,
    installed: bool,
    chosen: bool,
    recommended: bool,
}

pub fn statuses(conn: &rusqlite::Connection, data_dir: &Path) -> Vec<ModelStatus> {
    let chosen: Option<String> = settings::get(conn, SETTING).ok().flatten();
    REGISTRY
        .iter()
        .map(|m| ModelStatus {
            id: m.id,
            licence: m.licence,
            size_bytes: m.size_bytes,
            dims: m.dims,
            installed: installed(data_dir, m),
            chosen: chosen.as_deref() == Some(m.id),
            recommended: m.recommended,
        })
        .collect()
}

/// Download into a temporary file, verify, then move into place.
pub fn download(data_dir: &Path, m: &ModelInfo, progress: Arc<AtomicU64>) -> Result<(), String> {
    std::fs::create_dir_all(dir(data_dir)).map_err(|e| e.to_string())?;
    let dest = file(data_dir, m);
    let part = dest.with_extension("part");
    let response = ureq::get(m.url)
        .call()
        .map_err(|e| format!("Download failed: {e}. Check your internet connection and try again."))?;
    let mut reader = response.into_body().into_reader();
    let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total += n as u64;
        progress.store(total, Ordering::Relaxed);
    }
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    let sum = sha256_file(&part).map_err(|e| e.to_string())?;
    if sum != m.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "The downloaded file does not match the pinned checksum, so it was discarded. The model may have \
             changed upstream; this version of Crate Digger only accepts {}.",
            &m.sha256[..12]
        ));
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())
}
