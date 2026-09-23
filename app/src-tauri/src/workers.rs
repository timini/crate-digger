//! The background job handlers the app runs.

use std::sync::Arc;
use std::time::Duration;

use cd_core::acquisition::{AcquireHandler, ValidateHandler};
use cd_core::adapters::demo::{DemoAcquirer, DemoSource};
use cd_core::analysis::handler::{AnalysisHandler, ProcessAnalyzer};
use cd_core::analysis::runner::RunnerConfig;
use cd_core::analysis::FeatureVersion;
use cd_core::archive::ArchiveHandler;
use cd_core::discovery::DiscoverHandler;
use cd_core::jobs::worker::Handler;
use cd_core::library::{AudioProbe, ImportHandler};

use crate::probe::SymphoniaProbe;
use crate::state::AppState;

/// Connector name for the only acquisition source in milestone 1.
pub const DEMO_CONNECTOR: &str = "demo";

/// The embedding version this build produces.
pub fn analysis_version() -> FeatureVersion {
    cd_analyzer::embed::version()
}

/// Analysis runs in a copy of this executable started with the worker flag,
/// so a crash in decoding or analysis cannot take the app down.
pub fn analyzer() -> ProcessAnalyzer {
    let exe = std::env::current_exe().unwrap_or_else(|_| "crate-digger".into());
    let mut config = RunnerConfig::new(exe);
    config.args = vec![cd_analyzer::WORKER_FLAG.to_string()];
    ProcessAnalyzer {
        config,
        version: analysis_version(),
    }
}

/// Queue analysis for tracks that lack the current version.
pub fn plan_analysis(conn: &rusqlite::Connection) -> cd_core::Result<()> {
    let p = cd_core::analysis::store::plan(conn, &analysis_version(), cd_core::util::now_ms())?;
    if p.queued > 0 || p.needs_audio > 0 {
        tracing::info!(queued = p.queued, needs_audio = p.needs_audio, "planned analysis");
    }
    Ok(())
}

pub fn handlers(state: &AppState) -> Vec<Arc<dyn Handler>> {
    let probe: Arc<dyn AudioProbe> = Arc::new(SymphoniaProbe);
    vec![
        Arc::new(ImportHandler {
            probe: probe.clone(),
            after: Some(Arc::new(plan_analysis)),
        }),
        Arc::new(DiscoverHandler {
            source: Arc::new(DemoSource),
            acquirer_id: DEMO_CONNECTOR.into(),
        }),
        Arc::new(AcquireHandler {
            acquirer: Arc::new(DemoAcquirer::default()),
            staging_root: state.staging_dir(),
            probe: probe.clone(),
            poll: Duration::from_millis(500),
        }),
        Arc::new(ValidateHandler { probe }),
        Arc::new(AnalysisHandler {
            analyzer: Arc::new(analyzer()),
            after: None,
        }),
        Arc::new(ArchiveHandler {
            root: {
                let default = state.default_archive_dir.clone();
                Arc::new(move |conn| {
                    crate::state::archive_dir_setting(conn).unwrap_or_else(|| default.clone())
                })
            },
        }),
    ]
}
