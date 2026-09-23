//! Model files are only used when they match their pinned checksum.

use cd_analyzer::models::{self, REGISTRY};
use cd_core::analysis::protocol::ModelRef;

fn reference(id: &str, path: &std::path::Path, sha: &str) -> ModelRef {
    ModelRef {
        id: id.into(),
        path: path.to_string_lossy().into(),
        sha256: sha.into(),
    }
}

#[test]
fn a_tampered_model_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.onnx");
    std::fs::write(&path, b"not the real weights").unwrap();
    let m = &REGISTRY[0];
    let err = models::load(&reference(m.id, &path, m.sha256)).err().unwrap();
    assert!(err.contains("does not match its pinned checksum"), "{err}");
}

#[test]
fn unknown_models_and_unpinned_checksums_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.onnx");
    std::fs::write(&path, b"x").unwrap();
    assert!(models::load(&reference("mystery-model", &path, "00")).is_err());
    let m = &REGISTRY[0];
    let err = models::load(&reference(m.id, &path, "0000")).err().unwrap();
    assert!(err.contains("unpinned"), "{err}");
}

#[test]
fn every_model_version_names_its_weights_and_preprocessing() {
    for m in REGISTRY {
        let v = models::version(m);
        assert_eq!(v.model_id, m.id);
        assert_eq!(v.weights_checksum, format!("sha256:{}", m.sha256));
        assert_eq!(v.preprocessing_version, cd_analyzer::mel16k::PREPROCESSING);
        assert_eq!(m.sha256.len(), 64);
    }
}
