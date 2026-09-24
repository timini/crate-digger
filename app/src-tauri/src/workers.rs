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

/// Sample the ready queue and, with automatic discovery on, top it up.
fn keep_queue_full(state: &AppState) {
    let Ok(conn) = state.db() else { return };
    let now = cd_core::util::now_ms();
    let limits = cd_core::settings::limits(&conn).unwrap_or_default();
    if let Ok(b) = cd_core::replenish::buffer(&conn, &limits) {
        let _ = cd_core::replenish::sample(&conn, b.ready, now);
    }
    let demo =
        cd_core::settings::get_or(&conn, cd_core::settings::keys::DEMO_DISCOVERY, false).unwrap_or(false);
    let automatic = state.connections.read().unwrap().enabled;
    let connector = if demo { DEMO_CONNECTOR } else { LIVE_SOURCE };
    if demo || automatic {
        match cd_core::replenish::top_up(&conn, connector, &limits, now) {
            Ok(Some(_)) => {
                drop(conn);
                state.notify_workers();
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("could not top up the queue: {e}"),
        }
    }
}

/// Tracks that became ready since the last rerank have no queue place yet.
fn rank_new_arrivals(state: &AppState) {
    use cd_core::analysis::handler::Analyzer;
    let Ok(conn) = state.db() else { return };
    let waiting: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM candidate WHERE stage = 'ready' AND status = 'active' AND queue_rank IS NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    drop(conn);
    if waiting {
        let version = state.analyzer.version();
        state.spawn_background("rerank", move |conn| {
            if let Err(e) = cd_core::review::rerank(conn, &version) {
                tracing::warn!("rerank failed: {e}");
            }
        });
    }
}

/// Queues seed discovery every six hours while automatic discovery is on.
/// A run that cannot proceed records why, so nothing fails silently.
pub fn spawn_refresh(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(60));
        let state = app.state::<AppState>();
        rank_new_arrivals(&state);
        keep_queue_full(&state);
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

pub fn youtube(state: &AppState) -> cd_connectors::youtube::YouTube {
    cd_connectors::youtube::YouTube {
        secrets: state.secrets.clone(),
        transport: Arc::new(Http::default()),
    }
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
                .with_video_lookup()
                .route(live(state, PAGE_SOURCE), SOULSEEK_CONNECTOR)
                .with_video_lookup(),
        ),
        Arc::new(cd_core::youtube::YoutubeHandler {
            lookup: Arc::new(youtube(state)),
        }),
        Arc::new(AcquireHandler {
            acquirers: vec![
                Arc::new(DemoAcquirer::default()),
                Arc::new(cd_connectors::slskd::SoulseekAcquirer {
                    locate: state.soulseek.clone(),
                    model: {
                        let config = state.connections.clone();
                        let secrets = state.secrets.clone();
                        Arc::new(move || {
                            let c = config.read().unwrap().clone();
                            if c.llm_model.trim().is_empty() {
                                return None;
                            }
                            cd_connectors::llm::client(&c, &*secrets, Arc::new(Http::default())).ok()
                        })
                    },
                }),
            ],
            staging_root: state.staging_dir(),
            probe: probe.clone(),
            poll: Duration::from_millis(500),
            queued_watch: cd_core::acquisition::QUEUED_WATCH,
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
                    )?;
                    // With a fingerprint stored, the track can be identified.
                    cd_core::metadata_lookup::queue(conn, track, false, cd_core::util::now_ms()).map(|_| ())
                })),
            })
        },
        Arc::new(cd_core::metadata_lookup::MetadataHandler {
            lookup: Arc::new(cd_connectors::metadata::MetadataService {
                secrets: state.secrets.clone(),
                transport: Arc::new(Http::default()),
                limiter: Arc::default(),
                pace: true,
            }),
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
