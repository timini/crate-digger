# 0001: Audio analysis model

Status: accepted, 2026-09-23. Required before shared feature uploads (#18).

## Decision

Pin **Discogs-EffNet** (`discogs-effnet-bsdynamic-1`) as the recommended embedding model for similarity and ranking. Keep the built-in **cd-dsp-v1** summary as an always-computed baseline, the fallback when no model is installed, and the backend used in CI.

The model is not bundled. Users download it from Settings, which verifies it against the pinned checksum; the worker verifies it again before every load.

## Pinned version

| Item | Value |
| --- | --- |
| Model | `discogs-effnet-bsdynamic-1`, Essentia model catalogue (MTG, Universitat Pompeu Fabra) |
| Weights | https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.onnx |
| SHA-256 | `a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c` (18,027,718 bytes) |
| Licence | CC BY-NC-SA 4.0: allowed in this free, non-commercial app; not usable in a commercial product or service |
| Output | `embeddings`, 1280 dimensions per 128-frame patch |
| Preprocessing (`essentia-musicnn-input-16k-v1`) | Mono, resampled to 16 kHz; 512-sample symmetric Hann frames, 256-sample hop; magnitude spectrum; 96 Slaney mel bands from 0 to 8 kHz with unit-area triangles; log10(1 + 10000 x). Reimplemented in Rust (`crates/analyzer/src/mel16k.rs`) from Essentia's TensorflowInputMusiCNN. |
| Patches | 128 frames (2.05 s) with a 64-frame hop, averaged per segment |
| Segment policy | Three 30 s windows centred at 25, 50 and 75% of the track (the whole track when shorter than 90 s), plus their mean as the track summary |
| Runtime | tract-onnx 0.23.8, pure Rust, CPU, in the isolated analysis worker |
| Feature version | model `discogs-effnet-bsdynamic-1`, weights `sha256:a280825b…e43c`, preprocessing `essentia-musicnn-input-16k-v1` |

## Evidence

Evaluated on 157 tracks sampled from the maintainer's DJ library (house and techno; MP3, FLAC, AIFF), middle two minutes of each. Full results: [embeddings-2026-09-23](../evaluation/embeddings-2026-09-23.md).

| Backend | Same recording vs random (AUC) | d′ | Sections of one track vs other tracks (AUC) | Same recording vs same artist (AUC) |
| --- | --- | --- | --- | --- |
| cd-dsp-v1 | 0.996 | 2.11 | 0.777 | 0.993 |
| Discogs-EffNet | 0.997 | 2.85 | 0.941 | 0.981 |
| MusiCNN | 0.997 | 2.08 | 0.863 | 0.992 |

- All three recognise the same recording across a remaster, an MP3 re-encode and ±4% speed. That task mostly tests robustness, and they tie.
- Different sections of one track are the better test of musical similarity: EffNet places them far closer together than other tracks (0.941), clearly ahead of MusiCNN and the baseline. It also has the widest separation (d′ 2.85).
- EffNet rates tracks by the same artist as closer than the others do (0.981), which suits recommendation: related music should sit nearby.
- Only three different-mix pairs were present, too few to judge how well versions are told apart. Identity never relies on embeddings anyway; fingerprints decide.

## Cost

Measured with `crates/analyzer/examples/bench.rs`, one worker process per file, on an Apple M4 (macOS 26.6), 19 full-length tracks (124 minutes of audio):

| Backends | Seconds per audio minute | Median seconds per track | Peak memory |
| --- | --- | --- | --- |
| cd-dsp-v1 only | 0.15 | 0.9 | 24 MB |
| cd-dsp-v1 + EffNet | 0.44 | 2.8 | 125 MB |
| cd-dsp-v1 + MusiCNN | 0.29 | 1.8 | 75 MB |

A 1,000-track library takes about 45 minutes of background analysis with EffNet at the default of one analysis at a time. Results on the release reference hardware go in the release validation report (#21).

## Tempo and key

On the same sample: tempo matched tagged BPM within 2% on 24 of 25 tracks. Key matched only 5 of 12 tagged keys exactly (MIREX score 0.60). Key estimation needs improvement before it is presented with confidence; tagged keys from DJ software take precedence in the library already.

## Limits and follow-up

- The preprocessing is a reimplementation, not Essentia itself, and has not been checked value-for-value against Essentia's output. The evaluation shows the embeddings behave as expected, but a direct comparison should be run before shared uploads, since shared features must match across clients.
- MAEST, a stronger Discogs model, has no published ONNX export at the URLs checked; OpenL3's input pipeline could not be reproduced reliably. Neither was evaluated.
- Recommendation quality is not measured here. That needs held-out ratings (#13, #22).
