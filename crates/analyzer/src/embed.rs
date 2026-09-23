//! The `cd-dsp-v1` embedding: a fixed, hand-built summary of a segment's
//! spectrum, harmony and rhythm. It needs no model download and has no
//! licence restrictions, so it is the baseline and the CI backend.

use cd_core::analysis::FeatureVersion;

use crate::frames::{Frame, MEL_BANDS};

pub fn version() -> FeatureVersion {
    FeatureVersion {
        model_id: "cd-dsp-v1".into(),
        weights_checksum: "none".into(),
        preprocessing_version: "stft-2048-512-mel32-v1".into(),
    }
}

fn mean_std(values: impl Iterator<Item = f32> + Clone) -> (f32, f32) {
    let n = values.clone().count().max(1) as f32;
    let mean = values.clone().sum::<f32>() / n;
    let var = values.map(|v| (v - mean).powi(2)).sum::<f32>() / n;
    (mean, var.sqrt())
}

fn l2_normalise(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Weighted groups, each normalised so no group dominates.
fn push_group(out: &mut Vec<f32>, mut group: Vec<f32>, weight: f32) {
    l2_normalise(&mut group);
    out.extend(group.into_iter().map(|x| x * weight));
}

pub fn embed(frames: &[Frame]) -> Vec<f32> {
    let voiced: Vec<&Frame> = frames.iter().filter(|f| f.rms > 1e-4).collect();
    let frames: Vec<&Frame> = if voiced.len() > 10 {
        voiced
    } else {
        frames.iter().collect()
    };
    let mut out = Vec::with_capacity(96);

    let mut mel_mean = Vec::with_capacity(MEL_BANDS);
    let mut mel_std = Vec::with_capacity(MEL_BANDS);
    for b in 0..MEL_BANDS {
        let (m, s) = mean_std(frames.iter().map(|f| f.log_mel[b]));
        mel_mean.push(m);
        mel_std.push(s);
    }
    // Spectral shape, independent of overall level.
    let level = mel_mean.iter().sum::<f32>() / MEL_BANDS as f32;
    mel_mean.iter_mut().for_each(|m| *m -= level);
    push_group(&mut out, mel_mean, 1.0);
    push_group(&mut out, mel_std, 0.7);

    let mut c_mean = Vec::with_capacity(12);
    let mut c_std = Vec::with_capacity(12);
    for p in 0..12 {
        let (m, s) = mean_std(frames.iter().map(|f| f.chroma[p]));
        c_mean.push(m);
        c_std.push(s);
    }
    push_group(&mut out, c_mean, 0.8);
    push_group(&mut out, c_std, 0.4);

    let (om, os) = mean_std(frames.iter().map(|f| f.onset));
    let (cm, cs) = mean_std(frames.iter().map(|f| f.centroid));
    let (rm, rs) = mean_std(frames.iter().map(|f| f.rolloff));
    let (fm, fs) = mean_std(frames.iter().map(|f| f.flatness));
    push_group(
        &mut out,
        vec![om, os, cm * 4.0, cs * 4.0, rm * 2.0, rs * 2.0, fm, fs],
        0.6,
    );

    l2_normalise(&mut out);
    out
}

/// Mean of segment embeddings, renormalised.
pub fn mean(vectors: &[Vec<f32>]) -> Vec<f32> {
    let dims = vectors.first().map(|v| v.len()).unwrap_or(0);
    let mut m = vec![0f32; dims];
    for v in vectors {
        for (a, b) in m.iter_mut().zip(v) {
            *a += b;
        }
    }
    l2_normalise(&mut m);
    m
}
