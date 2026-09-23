//! The background job handlers the app runs.

use std::sync::Arc;
use std::time::Duration;

use cd_core::acquisition::{AcquireHandler, AnalyseHandler, ValidateHandler};
use cd_core::adapters::demo::{DemoAcquirer, DemoSource};
use cd_core::discovery::DiscoverHandler;
use cd_core::jobs::worker::Handler;
use cd_core::library::{AudioProbe, ImportHandler};

use crate::probe::SymphoniaProbe;
use crate::state::AppState;

/// Connector name for the only acquisition source in milestone 1.
pub const DEMO_CONNECTOR: &str = "demo";

pub fn handlers(state: &AppState) -> Vec<Arc<dyn Handler>> {
    let probe: Arc<dyn AudioProbe> = Arc::new(SymphoniaProbe);
    vec![
        Arc::new(ImportHandler { probe: probe.clone() }),
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
        Arc::new(ValidateHandler { probe: probe.clone() }),
        Arc::new(AnalyseHandler { probe }),
    ]
}
