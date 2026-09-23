//! Pretrained embedding models: the pinned registry and inference.
//!
//! Weights are downloaded separately (never committed) and verified against
//! the pinned SHA-256 before every load.

use std::io::Read;
use std::path::Path;

use cd_core::analysis::protocol::ModelRef;
use cd_core::analysis::FeatureVersion;
use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;

use crate::mel16k::{self, BANDS};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    pub licence: &'static str,
    /// Frames per input patch.
    pub patch_frames: usize,
    /// Frames between patch starts.
    pub patch_hop: usize,
    /// Whether to fix the input shape before loading. EffNet declares a
    /// symbolic batch size that tract resolves at run time; MusiCNN needs
    /// its batch dimension fixed to 1.
    pub fix_input_shape: bool,
    pub output: &'static str,
    pub dims: usize,
}

pub const REGISTRY: &[ModelInfo] = &[
    ModelInfo {
        id: "discogs-effnet-bsdynamic-1",
        url: "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.onnx",
        sha256: "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c",
        size_bytes: 18_027_718,
        licence: "CC BY-NC-SA 4.0 (Essentia models, MTG-UPF)",
        patch_frames: 128,
        patch_hop: 64,
        fix_input_shape: false,
        output: "embeddings",
        dims: 1280,
    },
    ModelInfo {
        id: "msd-musicnn-1",
        url: "https://essentia.upf.edu/models/feature-extractors/musicnn/msd-musicnn-1.onnx",
        sha256: "49668ffec47e52e94b96f45930bb46a28a1368d4bdfb5c05378fa834aca616e1",
        size_bytes: 3_168_334,
        licence: "CC BY-NC-SA 4.0 (Essentia models, MTG-UPF)",
        patch_frames: 187,
        patch_hop: 93,
        fix_input_shape: true,
        output: "embeddings",
        dims: 200,
    },
];

pub fn info(id: &str) -> Option<&'static ModelInfo> {
    REGISTRY.iter().find(|m| m.id == id)
}

pub fn version(m: &ModelInfo) -> FeatureVersion {
    FeatureVersion {
        model_id: m.id.to_string(),
        weights_checksum: format!("sha256:{}", m.sha256),
        preprocessing_version: mel16k::PREPROCESSING.to_string(),
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

type Plan = std::sync::Arc<TypedRunnableModel>;

pub struct LoadedModel {
    pub info: &'static ModelInfo,
    plan: Plan,
}

/// Verify and load a model. Refuses unknown models and files whose
/// checksum does not match the pinned value.
pub fn load(r: &ModelRef) -> Result<LoadedModel, String> {
    let info = info(&r.id).ok_or_else(|| format!("unknown model {}", r.id))?;
    if r.sha256 != info.sha256 {
        return Err(format!("model {} requested with an unpinned checksum", r.id));
    }
    let actual = sha256_file(Path::new(&r.path)).map_err(|e| format!("cannot read model {}: {e}", r.path))?;
    if actual != info.sha256 {
        return Err(format!(
            "model file {} does not match its pinned checksum; delete it and download it again",
            r.path
        ));
    }
    let fail = |e: TractError| format!("cannot load model {}: {e}", r.id);
    let mut model = tract_onnx::onnx().model_for_path(&r.path).map_err(fail)?;
    if info.fix_input_shape {
        model = model
            .with_input_fact(0, f32::fact([1, info.patch_frames, BANDS]).into())
            .map_err(fail)?;
    }
    let outlet = model
        .find_outlet_label(info.output)
        .ok_or_else(|| format!("model {} has no output named {}", r.id, info.output))?;
    model.select_output_outlets(&[outlet]).map_err(fail)?;
    let plan = model
        .into_optimized()
        .and_then(|m| m.into_runnable())
        .map_err(fail)?;
    Ok(LoadedModel { info, plan })
}

impl LoadedModel {
    pub fn version(&self) -> FeatureVersion {
        version(self.info)
    }

    /// Mean embedding over the patches of `frames` (mel rows).
    pub fn embed(&self, frames: &[[f32; BANDS]]) -> Result<Option<Vec<f32>>, String> {
        let n = self.info.patch_frames;
        if frames.len() < n {
            return Ok(None);
        }
        let mut sum = vec![0f32; self.info.dims];
        let mut count = 0;
        let mut start = 0;
        while start + n <= frames.len() {
            let data: Vec<f32> = frames[start..start + n].iter().flatten().copied().collect();
            let input = Tensor::from_shape(&[1, n, BANDS], &data).map_err(|e| e.to_string())?;
            let out = self.plan.run(tvec!(input.into())).map_err(|e| e.to_string())?;
            let view = out[0].to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
            for (s, v) in sum.iter_mut().zip(view.iter()) {
                *s += v;
            }
            count += 1;
            start += self.info.patch_hop;
        }
        sum.iter_mut().for_each(|v| *v /= count as f32);
        Ok(Some(sum))
    }
}
