// An in-process analyzer for core tests: real fingerprints and waveforms
// from the audio crate, and a small made-up embedding.

use std::path::Path;

use cd_core::analysis::handler::Analyzer;
use cd_core::analysis::protocol::{
    Analysis, EmbeddingOut, ErrorKind, FingerprintOut, Quality, Segment, Stats,
};
use cd_core::analysis::runner::WorkerError;
use cd_core::analysis::FeatureVersion;

pub struct FakeAnalyzer {
    pub version: FeatureVersion,
}

impl Default for FakeAnalyzer {
    fn default() -> Self {
        FakeAnalyzer {
            version: FeatureVersion {
                model_id: "fake-v1".into(),
                weights_checksum: "none".into(),
                preprocessing_version: "p1".into(),
            },
        }
    }
}

fn fail(e: cd_audio::AudioError) -> WorkerError {
    let kind = match e {
        cd_audio::AudioError::NotFound(_) => ErrorKind::NotFound,
        cd_audio::AudioError::Unreadable { .. } => ErrorKind::Unreadable,
        cd_audio::AudioError::Unsupported(_) => ErrorKind::Unsupported,
        cd_audio::AudioError::Corrupt(_) => ErrorKind::Corrupt,
    };
    WorkerError::Failed {
        kind,
        message: e.user_message(),
    }
}

impl Analyzer for FakeAnalyzer {
    fn analyse(&self, path: &Path) -> Result<Analysis, WorkerError> {
        let report = cd_audio::decode::decode_all(path).map_err(fail)?;
        let fp = self.fingerprint(path, 1.0)?;
        let d = report.decoded_ms as f32;
        Ok(Analysis {
            duration_ms: report.decoded_ms,
            sample_rate: report.info.sample_rate,
            channels: report.info.channels,
            fingerprint: fp,
            tempo_bpm: Some(124.0),
            tempo_confidence: 0.9,
            key: None,
            loudness_lufs: Some(-12.0),
            quality: Quality {
                decoded_fraction: Some(1.0),
                clipping_ratio: 0.0,
                silence_ratio: 0.0,
                decode_errors: 0,
            },
            segments: vec![Segment {
                start_ms: 0,
                end_ms: report.decoded_ms,
            }],
            embeddings: vec![
                EmbeddingOut {
                    version: self.version.clone(),
                    segment: Some(0),
                    vector: vec![1.0, d / 1000.0, 0.5],
                },
                EmbeddingOut {
                    version: self.version.clone(),
                    segment: None,
                    vector: vec![1.0, d / 1000.0, 0.5],
                },
            ],
            waveform: cd_audio::waveform::peaks(path, 800).map_err(fail)?,
            stats: Stats {
                wall_ms: 1,
                peak_rss_kb: 1,
            },
        })
    }

    fn fingerprint(&self, path: &Path, speed: f64) -> Result<FingerprintOut, WorkerError> {
        let f = cd_audio::fingerprint::fingerprint_file(path, speed).map_err(fail)?;
        Ok(FingerprintOut {
            algorithm: f.algorithm,
            data: f.data,
            duration_ms: f.duration_ms,
            speed: f.speed,
        })
    }

    fn version(&self) -> FeatureVersion {
        self.version.clone()
    }
}
