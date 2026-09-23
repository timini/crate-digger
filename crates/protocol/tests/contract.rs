//! The JSON form of every v1 message, pinned in tests/v1. The service
//! repository runs the same checks against the same files.
//! Run with UPDATE_CONTRACT=1 only for a deliberate format change.

use cd_protocol::backup::{Playlist, Rating, Seed, Snapshot, Track, SNAPSHOT_VERSION};
use cd_protocol::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

fn effnet() -> FeatureVersion {
    let (v, _) = SHARED_FEATURE_VERSIONS[0];
    FeatureVersion {
        model_id: v.model_id.into(),
        weights_checksum: v.weights_checksum.into(),
        preprocessing_version: v.preprocessing_version.into(),
    }
}

fn key() -> RecordingKey {
    RecordingKey {
        fingerprint_hash: Some("fp:3f9a0c12".into()),
        external_ids: vec![ExternalId {
            source: "discogs_release".into(),
            id: "36471139".into(),
        }],
    }
}

fn metadata() -> Metadata {
    Metadata {
        artist: Some("Kerri Chandler".into()),
        title: Some("House Is House".into()),
        mix: None,
        label: Some("Kerri Chandler".into()),
        release: Some("Downtown EP Pt. 2".into()),
        year: Some(2025),
        duration_ms: Some(392_000),
    }
}

fn contribution() -> Contribution {
    Contribution {
        idempotency_key: "c-018f2a7e-1".into(),
        recording: key(),
        metadata: Some(metadata()),
        features: Some(Features {
            version: effnet(),
            embedding: vec![0.25; 1280],
            tempo_bpm: Some(124.0),
            key_camelot: Some("8A".into()),
            loudness_lufs: Some(-9.5),
        }),
        references: vec![Reference {
            kind: "youtube".into(),
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
        }],
        correction: false,
    }
}

fn entry() -> CatalogueEntry {
    CatalogueEntry {
        recording_id: "r-01".into(),
        metadata: metadata(),
        alternatives: vec![Metadata {
            mix: Some("Original Mix".into()),
            ..metadata()
        }],
        feature_versions: vec![effnet()],
        references: vec![],
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        version: SNAPSHOT_VERSION,
        created_at_ms: 1_790_164_800_000,
        tracks: vec![Track {
            id: "t1".into(),
            metadata: metadata(),
            fingerprint_hash: Some("fp:3f9a0c12".into()),
            kept: true,
        }],
        ratings: vec![Rating {
            track: "t1".into(),
            kind: "star3".into(),
            at_ms: 1_790_164_800_000,
        }],
        seeds: vec![Seed {
            kind: "artist".into(),
            value: "Kerri Chandler".into(),
        }],
        playlists: vec![Playlist {
            name: "Friday".into(),
            tracks: vec!["t1".into()],
        }],
    }
}

/// Keys sorted, whatever order serde_json keeps in this build.
fn canonical(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let sorted: std::collections::BTreeMap<String, serde_json::Value> =
                m.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(a.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Compares with the pinned file, then checks it parses back to the same value.
fn pinned<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(name: &str, value: &T) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/v1")
        .join(format!("{name}.json"));
    let mut json = serde_json::to_value(value).unwrap();
    // Keep the pinned files readable: shorten long embeddings to their length.
    if let Some(v) = json.pointer_mut("/contributions/0/features/embedding") {
        *v = serde_json::json!(format!("{} values", v.as_array().unwrap().len()));
    }
    let text = serde_json::to_string_pretty(&canonical(json)).unwrap() + "\n";
    if std::env::var("UPDATE_CONTRACT").is_ok() {
        std::fs::write(&path, &text).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(text, expected, "{name}: the v1 wire format changed");
    let back: T = serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap();
    assert_eq!(&back, value);
}

#[test]
fn v1_messages_keep_their_json_form() {
    pinned(
        "submit_request",
        &SubmitRequest {
            contributions: vec![contribution()],
        },
    );
    pinned(
        "submit_response",
        &SubmitResponse {
            acks: vec![
                Ack::Accepted {
                    idempotency_key: "c-1".into(),
                },
                Ack::Duplicate {
                    idempotency_key: "c-2".into(),
                },
                Ack::Rejected {
                    idempotency_key: "c-3".into(),
                    reason: "wrong_dimensions".into(),
                },
            ],
        },
    );
    pinned(
        "lookup_request",
        &LookupRequest {
            recordings: vec![key()],
        },
    );
    pinned(
        "lookup_response",
        &LookupResponse {
            results: vec![Some(entry()), None],
        },
    );
    pinned(
        "changes_request",
        &ChangesRequest {
            cursor: Some("c:42".into()),
            limit: 50,
        },
    );
    pinned(
        "changes_response",
        &ChangesResponse {
            changes: vec![entry()],
            next_cursor: None,
        },
    );
    pinned("backup_snapshot", &snapshot());
}

#[test]
fn shared_messages_have_no_field_for_private_data() {
    for file in [
        "submit_request",
        "lookup_request",
        "lookup_response",
        "changes_response",
    ] {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/v1/{file}.json")),
        )
        .unwrap();
        for private in ["path", "rating", "password", "token", "api_key", "session"] {
            assert!(
                !text.contains(&format!("\"{private}")),
                "{file} has a {private} field"
            );
        }
    }
    // Unknown fields, such as a smuggled local path, are refused.
    let mut v = serde_json::to_value(contribution()).unwrap();
    v["path"] = serde_json::json!("/Users/dj/Music/x.flac");
    assert!(serde_json::from_value::<Contribution>(v).is_err());
}

#[test]
fn contributions_are_validated() {
    assert!(contribution().validate().is_ok());
    let code = |c: Contribution| c.validate().unwrap_err().code;

    let mut c = contribution();
    c.features.as_mut().unwrap().embedding.pop();
    assert_eq!(code(c), "wrong_dimensions");

    let mut c = contribution();
    c.features.as_mut().unwrap().version.model_id = "cd-dsp-v1".into();
    assert_eq!(code(c), "unsupported_model");

    let mut c = contribution();
    c.features.as_mut().unwrap().embedding[3] = f32::NAN;
    assert_eq!(code(c), "not_finite");

    let mut c = contribution();
    c.metadata.as_mut().unwrap().title = Some("x".repeat(MAX_TEXT + 1));
    assert_eq!(code(c), "invalid_text");

    let mut c = contribution();
    c.recording = RecordingKey {
        fingerprint_hash: None,
        external_ids: vec![],
    };
    assert_eq!(code(c), "no_key");

    let mut c = contribution();
    c.references[0].url = "https://evil.example/watch?v=x".into();
    assert_eq!(code(c), "bad_reference");

    let mut c = contribution();
    c.idempotency_key = "has spaces".into();
    assert_eq!(code(c), "bad_idempotency_key");

    let too_many = SubmitRequest {
        contributions: vec![contribution(); MAX_BATCH + 1],
    };
    assert_eq!(too_many.validate().unwrap_err().code, "batch_size");
}

#[test]
fn snapshots_are_validated() {
    assert!(snapshot().validate().is_ok());
    let mut s = snapshot();
    s.ratings[0].track = "missing".into();
    assert_eq!(s.validate().unwrap_err().code, "unknown_track");
    let mut s = snapshot();
    s.ratings[0].kind = "skip".into();
    assert_eq!(s.validate().unwrap_err().code, "rating_kind");
}
