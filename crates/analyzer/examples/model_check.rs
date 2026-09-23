//! Load each registered model from a folder and embed a synthetic song:
//! model_check <models folder>
use cd_analyzer::models::{self, REGISTRY};
use cd_core::analysis::protocol::ModelRef;

fn main() {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: model_check <models folder>"),
    );
    let tmp = std::env::temp_dir().join("cd-model-check.wav");
    cd_audio::synth::Song::from_seed(3)
        .render(100.0)
        .write_wav(&tmp)
        .unwrap();
    for info in REGISTRY {
        let r = ModelRef {
            id: info.id.into(),
            path: dir.join(format!("{}.onnx", info.id)).to_string_lossy().into(),
            sha256: info.sha256.into(),
        };
        let t = std::time::Instant::now();
        let m = match models::load(&r) {
            Ok(m) => m,
            Err(e) => {
                println!("{}: {e}", info.id);
                continue;
            }
        };
        let load_ms = t.elapsed().as_millis();
        let t = std::time::Instant::now();
        let a = cd_analyzer::analyse(&tmp, &[m], &|_| {}).unwrap();
        let emb: Vec<_> = a
            .embeddings
            .iter()
            .filter(|e| e.version.model_id == info.id)
            .collect();
        let v = &emb.last().unwrap().vector;
        println!(
            "{}: load {load_ms} ms, analyse 100 s in {} ms, {} vectors of {} dims, peak {} KB, first values {:?}",
            info.id,
            t.elapsed().as_millis(),
            emb.len(),
            v.len(),
            a.stats.peak_rss_kb,
            &v[..4]
        );
    }
}
