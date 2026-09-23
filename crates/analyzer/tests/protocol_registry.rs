//! The model accepted for sharing must be exactly the pinned one here.
use cd_analyzer::models::{version, REGISTRY};

#[test]
fn shared_model_matches_the_pinned_analysis_model() {
    let pinned = REGISTRY
        .iter()
        .find(|m| m.recommended)
        .expect("a recommended model");
    let v = version(pinned);
    let shared = cd_protocol::FeatureVersion {
        model_id: v.model_id,
        weights_checksum: v.weights_checksum,
        preprocessing_version: v.preprocessing_version,
    };
    assert_eq!(shared.shared_dims(), Some(pinned.dims));
    assert_eq!(
        cd_protocol::SHARED_FEATURE_VERSIONS.len(),
        1,
        "only the pinned model is shared"
    );
}
