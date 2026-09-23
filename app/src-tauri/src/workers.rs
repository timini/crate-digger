//! The background job handlers the app runs.

use std::sync::Arc;
use std::time::Duration;

use cd_connectors::discovery::{LiveSource, LIVE_SOURCE, PAGE_SOURCE};
use cd_connectors::http::Http;
use cd_core::acquisition::{AcquireHandler, ValidateHandler};
use cd_core::adapters::demo::{DemoAcquirer, DemoSource};
use cd_core::analysis::handler::{AnalysisHandler, Analyzer, ProcessAnalyzer};
use cd_core::analysis::protocol::ModelRef;
use cd_core::analysis::runner::RunnerConfig;
use cd_core::analysis::FeatureVersion;
use cd_core::archive::ArchiveHandler;
use cd_core::discovery::DiscoverHandler;
use cd_core::jobs::worker::Handler;
use cd_core::library::{AudioProbe, ImportHandler};

use crate::probe::SymphoniaProbe;
use crate::state::AppState;

/// Connector name for demo discovery and its generated audio.
pub const DEMO_CONNECTOR: &str = "demo";
/// Connector name for Soulseek downloads through slskd.
pub const SOULSEEK_CONNECTOR: &str = "slskd";
/// Seed discovery runs this often while the app is open and it is turned on.
pub const REFRESH_INTERVAL_MS: i64 = 6 * 3_600_000;

/// Queues seed discovery every six hours while automatic discovery is on.
/// A run that cannot proceed records why, so nothing fails silently.
pub fn spawn_refresh(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(60));
        let state = app.state::<AppState>();
        if !state.connections.read().unwrap().enabled {
            continue;
        }
        let Ok(conn) = state.db() else { continue };
        let now = cd_core::util::now_ms();
        match cd_core::discovery::refresh_due(&conn, LIVE_SOURCE, REFRESH_INTERVAL_MS, now) {
            Ok(true) => {
                if let Err(e) = cd_core::discovery::request_discovery(&conn, LIVE_SOURCE, 20, now) {
                    tracing::warn!("could not queue discovery: {e}");
                }
                drop(conn);
                state.notify_workers();
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("could not check discovery refresh: {e}"),
        }
    });
}

/// Analysis runs in a copy of this executable started with the worker flag,
/// so a crash in decoding or analysis cannot take the app down. The chosen
/// pretrained model, if installed, runs alongside the built-in baseline and
/// its embeddings drive ranking.
pub fn analyzer(model: Option<ModelRef>) -> ProcessAnalyzer {
    let exe = std::env::current_exe().unwrap_or_else(|_| "crate-digger".into());
    let mut config = RunnerConfig::new(exe);
    config.args = vec![cd_analyzer::WORKER_FLAG.to_string()];
    let version = model
        .as_ref()
        .and_then(|m| cd_analyzer::models::info(&m.id))
        .map(cd_analyzer::models::version)
        .unwrap_or_else(cd_analyzer::embed::version);
    ProcessAnalyzer {
        config,
        version,
        models: model.into_iter().collect(),
    }
}

/// Queue analysis for tracks that lack the current version.
pub fn plan_analysis(conn: &rusqlite::Connection, version: &FeatureVersion) -> cd_core::Result<()> {
    let p = cd_core::analysis::store::plan(conn, version, cd_core::util::now_ms())?;
    if p.queued > 0 || p.needs_audio > 0 {
        tracing::info!(queued = p.queued, needs_audio = p.needs_audio, "planned analysis");
    }
    Ok(())
}

fn live(state: &AppState, name: &'static str) -> Arc<LiveSource> {
    Arc::new(LiveSource {
        name,
        config: state.connections.clone(),
        secrets: state.secrets.clone(),
        transport: Arc::new(Http::default()),
    })
}

pub fn handlers(state: &AppState) -> Vec<Arc<dyn Handler>> {
    let probe: Arc<dyn AudioProbe> = Arc::new(SymphoniaProbe);
    vec![
        Arc::new(ImportHandler {
            probe: probe.clone(),
            after: Some({
                let analyzer = state.analyzer.clone();
                Arc::new(move |conn| plan_analysis(conn, &analyzer.version()))
            }),
        }),
        Arc::new(
            DiscoverHandler::new(Arc::new(DemoSource), DEMO_CONNECTOR)
                .route(live(state, LIVE_SOURCE), SOULSEEK_CONNECTOR)
                .route(live(state, PAGE_SOURCE), SOULSEEK_CONNECTOR),
        ),
        // Soulseek joins in #12; until then live candidates wait with a reason.
        Arc::new(AcquireHandler {
            acquirers: vec![Arc::new(DemoAcquirer::default())],
            staging_root: state.staging_dir(),
            probe: probe.clone(),
            poll: Duration::from_millis(500),
        }),
        Arc::new(ValidateHandler { probe }),
        {
            let analyzer = state.analyzer.clone();
            let for_matching = analyzer.clone();
            Arc::new(AnalysisHandler {
                analyzer,
                // Match each analysed file against the library.
                after: Some(Arc::new(move |conn, track, file| {
                    cd_core::identity::matching::match_track(
                        conn,
                        for_matching.as_ref(),
                        track,
                        file,
                        &Default::default(),
                    )
                    .map(|_| ())
                })),
            })
        },
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
