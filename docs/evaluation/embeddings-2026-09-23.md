<!-- Output of crates/analyzer/examples/evaluate.rs on the maintainer's library, 2026-09-23. Aggregates only. -->
# Embedding evaluation

157 tracks (middle 120 s of each), 3 backends. Variants: remaster, ±4% speed, MP3 128 kbps re-encode.

| Backend | Same recording vs random (AUC) | vs same artist (AUC) | TPR at 1% FPR | d′ | Pitch ±4% vs random (AUC) | Sections vs random (AUC) | Versions: mean sim | Versions vs random (AUC) | Same recording vs versions (AUC) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cd-dsp-v1 | 0.996 | 0.993 | 0.924 | 2.11 | 0.991 | 0.777 | 0.942 (n=3) | 0.656 | 0.992 |
| discogs-effnet-bsdynamic-1 | 0.997 | 0.981 | 0.846 | 2.85 | 0.994 | 0.941 | 0.828 (n=3) | 0.649 | 0.999 |
| msd-musicnn-1 | 0.997 | 0.992 | 0.959 | 2.08 | 0.994 | 0.863 | 0.934 (n=3) | 0.549 | 0.999 |

## Tempo and key against tags

- Tempo: 25 tracks with a tagged BPM; 24 within 2% (96%), 0 off by a factor of 2 or 1.5.
- Key: 12 tracks with a tagged key; 5 exact (42%), MIREX weighted score 0.60.

## Cost

Analysing one 120 s clip with all backends took 4497 ms on average (8 clips in parallel). Peak memory of the whole evaluation process: 3371 MB.
