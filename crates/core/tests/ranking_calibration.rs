//! Calibration of the ranking weights on synthetic listeners, because no
//! real ratings exist yet. Weights are chosen on one generated world and
//! reported on a second, independent one. Also times a rerank of 1,000
//! candidates.
//!
//! Set CALIBRATION_REPORT=<path> to write the results as Markdown. Run
//! with --release for timings that match an installed app.

use std::time::Instant;

use cd_core::analysis::{Embedding, FeatureVersion};
use cd_core::ranking::{order, score, Config, Item, Positive, Profile, Slot, DEFAULT};

const STYLES: usize = 6;

/// Small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn normal(&mut self) -> f32 {
        let (u, v) = (self.unit().max(1e-6), self.unit());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f32::consts::PI * v).cos()
    }
}

struct Track {
    style: usize,
    embedding: Embedding,
    evidence: f32,
    liked: bool,
}

/// A listener who loves styles 0 and 1, is lukewarm on 2, dislikes 3 and
/// has never heard 4 and 5. Sources favour styles 0, 1 and 4.
fn world(seed: u64, dims: usize, tracks: usize, v: &FeatureVersion) -> Vec<Track> {
    let mut rng = Rng(seed);
    let centres: Vec<Vec<f32>> = (0..STYLES)
        .map(|_| (0..dims).map(|_| rng.normal()).collect())
        .collect();
    (0..tracks)
        .map(|_| {
            let style = (rng.next() % STYLES as u64) as usize;
            let vector = centres[style].iter().map(|c| c + rng.normal() * 0.7).collect();
            let like_rate = [0.85, 0.85, 0.3, 0.05, 0.4, 0.2][style];
            let evidence_base = [0.6, 0.6, 0.4, 0.4, 0.6, 0.3][style];
            Track {
                style,
                embedding: Embedding::new(v.clone(), vector),
                evidence: (evidence_base + (rng.unit() - 0.5) * 0.5).clamp(0.0, 1.0),
                liked: rng.unit() < like_rate,
            }
        })
        .collect()
}

struct Metrics {
    precision_20: f32,
    precision_50: f32,
    /// Share of the smaller liked style in the personal top 20.
    minority_share: f32,
    disliked_in_top_20: usize,
}

fn evaluate(history: &[Track], pool: &[Track], cfg: &Config, rated: usize) -> Metrics {
    let positives = history
        .iter()
        .take(rated)
        .filter(|t| t.liked)
        .map(|t| Positive {
            embedding: t.embedding.unit(),
            weight: 1.0,
        })
        .collect();
    let dislikes = history
        .iter()
        .take(rated)
        .filter(|t| !t.liked)
        .map(|t| t.embedding.unit())
        .collect();
    let profile = Profile::build(positives, dislikes, cfg);
    let scored = pool
        .iter()
        .enumerate()
        .map(|(i, t)| {
            score(
                &profile,
                &Item {
                    id: i.to_string(),
                    embedding: Some(t.embedding.unit()),
                    evidence: t.evidence,
                    seed_match: false,
                    artist: String::new(),
                },
                cfg,
            )
        })
        .collect();
    let queue: Vec<usize> = order(scored, &profile, cfg)
        .into_iter()
        .filter(|(_, slot)| *slot == Slot::Personal)
        .map(|(s, _)| s.id.parse().unwrap())
        .collect();
    let precision = |n: usize| queue.iter().take(n).filter(|&&i| pool[i].liked).count() as f32 / n as f32;
    let top: Vec<&Track> = queue.iter().take(20).map(|&i| &pool[i]).collect();
    let (a, b) = (
        top.iter().filter(|t| t.style == 0).count(),
        top.iter().filter(|t| t.style == 1).count(),
    );
    Metrics {
        precision_20: precision(20),
        precision_50: precision(50),
        minority_share: a.min(b) as f32 / 20.0,
        disliked_in_top_20: top.iter().filter(|t| t.style == 3).count(),
    }
}

fn grid() -> Vec<Config> {
    let mut out = vec![];
    for w_taste in [0.6, 1.0, 1.4] {
        for w_evidence in [0.1, 0.3, 0.5] {
            for cluster_threshold in [0.3, 0.45, 0.6] {
                for cluster_repeat_cost in [0.04, 0.08, 0.15] {
                    for neighbours in [5, 9] {
                        out.push(Config {
                            w_taste,
                            w_evidence,
                            cluster_threshold,
                            cluster_repeat_cost,
                            neighbours,
                            ..DEFAULT
                        });
                    }
                }
            }
        }
    }
    out
}

#[test]
fn chosen_weights_generalise_and_keep_both_tastes() {
    let v = FeatureVersion {
        model_id: "synthetic".into(),
        weights_checksum: "none".into(),
        preprocessing_version: "none".into(),
    };
    let dims = 64;
    let rated = 60;
    let train: Vec<Vec<Track>> = (1..=5).map(|seed| world(seed, dims, 800, &v)).collect();
    let held_out: Vec<Vec<Track>> = (11..=15).map(|seed| world(seed, dims, 800, &v)).collect();
    let mean = |worlds: &[Vec<Track>], cfg: &Config| -> Metrics {
        let ms: Vec<Metrics> = worlds
            .iter()
            .map(|w| evaluate(&w[..rated], &w[rated..], cfg, rated))
            .collect();
        let n = ms.len() as f32;
        Metrics {
            precision_20: ms.iter().map(|m| m.precision_20).sum::<f32>() / n,
            precision_50: ms.iter().map(|m| m.precision_50).sum::<f32>() / n,
            minority_share: ms.iter().map(|m| m.minority_share).sum::<f32>() / n,
            disliked_in_top_20: ms.iter().map(|m| m.disliked_in_top_20).sum::<usize>() / ms.len(),
        }
    };
    let objective = |m: &Metrics| m.precision_20 + m.precision_50 + m.minority_share;

    // Choose on the training worlds only.
    let mut best: Option<(Config, f32)> = None;
    for cfg in grid() {
        let o = objective(&mean(&train, &cfg));
        if best.as_ref().is_none_or(|(_, b)| o > *b + 1e-6) {
            best = Some((cfg, o));
        }
    }
    let chosen = best.unwrap().0;

    let evidence_only = Config {
        w_taste: 0.0,
        w_dislike: 0.0,
        w_evidence: 1.0,
        ..DEFAULT
    };
    // One cluster holding every like: taste becomes closeness to all of them at once.
    let one_vector = Config {
        cluster_threshold: -1.0,
        top_k: usize::MAX,
        neighbours: usize::MAX,
        ..chosen
    };
    let report = |cfg: &Config| mean(&held_out, cfg);
    let (c, e, o) = (report(&chosen), report(&evidence_only), report(&one_vector));

    // Timing: 1,000 candidates against 300 positives at EffNet's 1,280 dimensions.
    let big = world(3, 1280, 1300, &v);
    let started = Instant::now();
    let history = &big[..300];
    let pool = &big[300..];
    let _ = evaluate(history, pool, &chosen, 300);
    let elapsed = started.elapsed();

    if let Ok(path) = std::env::var("CALIBRATION_REPORT") {
        let row = |name: &str, m: &Metrics| {
            format!(
                "| {name} | {:.2} | {:.2} | {:.2} | {} |",
                m.precision_20, m.precision_50, m.minority_share, m.disliked_in_top_20
            )
        };
        let md = format!(
            "Chosen on five training worlds: w_taste {}, w_evidence {}, cluster_threshold {}, cluster_repeat_cost {}, neighbours {}.\n\n\
             | Five held-out worlds (mean) | Precision at 20 | Precision at 50 | Smaller liked style in top 20 | Disliked style in top 20 |\n\
             | --- | --- | --- | --- | --- |\n{}\n{}\n{}\n\n\
             Scoring and ordering 1,000 candidates against 300 positives at 1,280 dimensions took {:.2} s ({} build).\n",
            chosen.w_taste,
            chosen.w_evidence,
            chosen.cluster_threshold,
            chosen.cluster_repeat_cost,
            chosen.neighbours,
            row("Chosen weights, taste clusters", &c),
            row("One average vector, same weights", &o),
            row("Cultural evidence only", &e),
            elapsed.as_secs_f64(),
            if cfg!(debug_assertions) { "debug" } else { "release" },
        );
        std::fs::write(path, md).unwrap();
    }

    assert_eq!(
        chosen, DEFAULT,
        "update ranking::DEFAULT to the calibrated weights"
    );
    assert!(
        c.precision_20 > e.precision_20,
        "personal ranking must beat evidence alone"
    );
    assert!(
        c.minority_share >= 0.2,
        "both liked styles must appear in the top 20"
    );
    assert!(c.minority_share >= o.minority_share);
    if !cfg!(debug_assertions) {
        assert!(elapsed.as_secs_f64() <= 2.0, "rerank took {elapsed:?}");
    }
}
