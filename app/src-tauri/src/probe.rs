//! Connects the core's decodability check to the audio crate.

use std::path::Path;

use cd_core::library::{AudioProbe, ProbeInfo};

pub struct SymphoniaProbe;

impl AudioProbe for SymphoniaProbe {
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
}
