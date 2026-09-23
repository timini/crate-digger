//! The real worker process driven by the job system and stored in the
//! database, end to end.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cd_core::analysis::handler::{AnalysisHandler, ProcessAnalyzer};
use cd_core::analysis::runner::RunnerConfig;
use cd_core::analysis::store;
use cd_core::domain::FileOrigin;
use cd_core::jobs::kinds;
use cd_core::jobs::scheduler::{staged_bytes, Limits, Scheduler};
use cd_core::jobs::worker::{run_one, Handler};
use cd_core::library::{self, AudioProbe, ProbeInfo};

struct Probe;

impl AudioProbe for Probe {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, String> {
        cd_audio::probe(path)
            .map(|i| ProbeInfo {
                codec: i.codec,
                duration_ms: i.duration_ms.map(|d| d as i64),
                sample_rate: Some(i.sample_rate as i64),
                channels: Some(i.channels as i64),
            })
            .map_err(|e| e.to_string())
    }
    fn is_supported(&self, path: &Path) -> bool {
        cd_audio::decode::is_supported_extension(path)
    }
    fn decode_full(&self, path: &Path) -> Result<i64, String> {
        cd_audio::decode::decode_all(path)
            .map(|r| r.decoded_ms as i64)
            .map_err(|e| e.to_string())
    }
    fn waveform(&self, path: &Path, bins: usize) -> Result<Vec<u8>, String> {
        cd_audio::waveform::peaks(path, bins).map_err(|e| e.to_string())
    }
}

#[test]
fn imported_file_is_analysed_by_the_worker_process() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.wav");
    cd_audio::synth::Song::from_seed(11)
        .render(100.0)
        .write_wav(&path)
        .unwrap();
    let mut conn = cd_core::db::open(&dir.path().join("db.sqlite")).unwrap();
    let r = library::register_file(&conn, &path, None, FileOrigin::Imported, &Probe).unwrap();

    let version = cd_analyzer::embed::version();
    assert_eq!(store::plan(&conn, &version, 1).unwrap().queued, 1);
    let analyzer = ProcessAnalyzer {
        config: RunnerConfig::new(PathBuf::from(env!("CARGO_BIN_EXE_cd-analyzer"))),
        version: version.clone(),
        models: vec![],
    };
    let h: Arc<dyn Handler> = Arc::new(AnalysisHandler {
        analyzer: Arc::new(analyzer),
        after: None,
    });
    let s = Scheduler::new(Limits::default(), Arc::new(staged_bytes));
    let handlers = HashMap::from([(h.kind(), h)]);
    assert!(run_one(
        &mut conn,
        &s,
        &handlers,
        &[kinds::ANALYSE],
        "w",
        &AtomicBool::new(false)
    )
    .unwrap());

    let e = store::summary_embedding(&conn, &r.track_id, &version)
        .unwrap()
        .unwrap();
    assert_eq!(e.dims(), 96);
    assert_eq!(
        store::status(&conn, &r.track_id, &version)
            .unwrap()
            .unwrap()
            .state,
        "done"
    );
    let m = cd_core::meta::effective(&conn, &r.track_id).unwrap();
    assert!((m.tempo.unwrap() - 125.0).abs() < 1.0, "{:?}", m.tempo);
    assert!(m.musical_key.is_some());
    let segments: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feature_record WHERE track_id = ?1 AND segment_index IS NOT NULL",
            [&r.track_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(segments, 3);
}
