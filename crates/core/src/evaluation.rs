//! The pilot report (#22, protocol in docs/pilot-evaluation.md).
//!
//! Ranking is compared by replay. Each discovered track's first judgement
//! is predicted twice, using only ratings made before it: once with
//! cultural evidence alone (Tier 1), and once with the full ranking
//! (Tier 1 and Tier 2). No judgement is ever predicted from a profile
//! that contains it.
//!
//! The report holds counts and rates only: no track names, ids, paths or
//! seeds, so a pilot DJ can share it as it is.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::analysis::FeatureVersion;
use crate::identity::normalize::fold;
use crate::ranking::{self, Config, Item, Positive, Profile};
use crate::Result;

pub const REPORT_VERSION: u32 = 1;

/// Judgements that count as a strong positive.
fn strong(kind: &str) -> bool {
    matches!(kind, "star2" | "star3")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// First judgements replayed.
    pub judgements: usize,
    pub strong_positives: usize,
    /// Judged tracks with no embedding of the current model; for them the
    /// full ranking falls back to cultural evidence.
    pub without_embedding: usize,
    /// Probability that a strong positive is ranked above another judged
    /// track (area under the ROC curve). None when there is nothing to compare.
    pub auc_cultural: Option<f64>,
    pub auc_combined: Option<f64>,
    /// 95% bootstrap interval for combined minus cultural.
    pub auc_difference_interval: Option<(f64, f64)>,
    /// Strong-positive rate among the top fifth of judged tracks by each score.
    pub top_fifth_strong_rate_cultural: Option<f64>,
    pub top_fifth_strong_rate_combined: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcomes {
    pub judged: usize,
    pub thumbs_down: usize,
    pub one_star: usize,
    pub two_stars: usize,
    pub three_stars: usize,
    pub skipped: usize,
    pub strong_positive_rate: Option<f64>,
    /// Judged tracks the DJ kept.
    pub keep_rate: Option<f64>,
    /// Judged tracks flagged as the wrong version.
    pub wrong_version_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stability {
    /// Pairs of copies of the same track analysed with the same model.
    pub pairs: usize,
    pub mean_similarity: Option<f64>,
    pub min_similarity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub report_version: u32,
    pub generated_at: i64,
    pub app_version: String,
    pub model_id: String,
    pub first_judgement_at: Option<i64>,
    pub last_judgement_at: Option<i64>,
    pub ranking: Comparison,
    pub outcomes: Outcomes,
    pub buffer: crate::replenish::Availability,
    pub source_runs: HashMap<String, i64>,
    pub analysis: HashMap<String, i64>,
    pub embedding_stability: Stability,
    /// Metrics the protocol lists that this app does not record.
    pub not_recorded: Vec<String>,
}

struct Judgement {
    track: String,
    kind: String,
    rowid: i64,
    at: i64,
}

fn first_judgements(conn: &Connection) -> Result<Vec<Judgement>> {
    // The first judgement of each discovered track that was not undone.
    let mut stmt = conn.prepare(
        "SELECT e.track_id, e.kind, e.rowid, e.created_at FROM rating_event e
         WHERE e.rowid = (
             SELECT MIN(f.rowid) FROM rating_event f
             WHERE f.track_id = e.track_id
               AND f.kind IN ('thumbs_down', 'star1', 'star2', 'star3', 'skip')
               AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = f.id))
           AND EXISTS (SELECT 1 FROM candidate c WHERE c.track_id = e.track_id)
         ORDER BY e.rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Judgement {
            track: r.get(0)?,
            kind: r.get(1)?,
            rowid: r.get(2)?,
            at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Effective ratings as they stood before event `rowid`, most recent last,
/// limited like the live ranking to the latest 500.
fn ratings_before(conn: &Connection, rowid: i64) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, kind FROM (
             SELECT e.track_id, e.kind, e.rowid AS r,
                    ROW_NUMBER() OVER (PARTITION BY e.track_id ORDER BY e.rowid DESC) AS rn
             FROM rating_event e
             WHERE e.rowid < ?1 AND e.kind IN ('thumbs_down', 'star1', 'star2', 'star3', 'cleared')
               AND NOT EXISTS (SELECT 1 FROM rating_event u WHERE u.undoes_event_id = e.id AND u.rowid < ?1)
         ) WHERE rn = 1 AND kind != 'cleared' ORDER BY r DESC LIMIT 500",
    )?;
    let mut rows = stmt
        .query_map(params![rowid], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// Area under the ROC curve, with ties counted as half.
pub fn auc(scored: &[(f32, bool)]) -> Option<f64> {
    let pos = scored.iter().filter(|s| s.1).count();
    let neg = scored.len() - pos;
    if pos == 0 || neg == 0 {
        return None;
    }
    let mut sorted: Vec<(f32, bool)> = scored.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Sum of positive ranks, averaging ranks within ties.
    let (mut rank_sum, mut i) = (0f64, 0);
    while i < sorted.len() {
        let mut j = i;
        while j < sorted.len() && sorted[j].0 == sorted[i].0 {
            j += 1;
        }
        let mean_rank = (i + 1 + j) as f64 / 2.0;
        rank_sum += mean_rank * sorted[i..j].iter().filter(|s| s.1).count() as f64;
        i = j;
    }
    Some((rank_sum - (pos * (pos + 1)) as f64 / 2.0) / (pos * neg) as f64)
}

fn top_fifth_rate(scored: &[(f32, bool)]) -> Option<f64> {
    if scored.len() < 5 {
        return None;
    }
    let mut sorted = scored.to_vec();
    // Highest first; equal scores keep judgement order.
    sorted.sort_by(|a, b| b.0.total_cmp(&a.0));
    let n = sorted.len() / 5;
    Some(sorted[..n].iter().filter(|s| s.1).count() as f64 / n as f64)
}

/// 95% interval of AUC(combined) minus AUC(cultural) over 1,000 resamples
/// of the judgements, with a fixed seed.
fn bootstrap_difference(pairs: &[(f32, f32, bool)]) -> Option<(f64, f64)> {
    let mut state = 0x2545f4914f6cdd1du64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let mut diffs = vec![];
    for _ in 0..1000 {
        let sample: Vec<(f32, f32, bool)> = (0..pairs.len()).map(|_| pairs[next(pairs.len())]).collect();
        let c: Vec<(f32, bool)> = sample.iter().map(|p| (p.0, p.2)).collect();
        let m: Vec<(f32, bool)> = sample.iter().map(|p| (p.1, p.2)).collect();
        if let (Some(a), Some(b)) = (auc(&c), auc(&m)) {
            diffs.push(b - a);
        }
    }
    if diffs.len() < 100 {
        return None;
    }
    diffs.sort_by(f64::total_cmp);
    let at = |q: f64| diffs[((diffs.len() - 1) as f64 * q).round() as usize];
    Some((at(0.025), at(0.975)))
}

fn rate(n: usize, of: usize) -> Option<f64> {
    (of > 0).then(|| n as f64 / of as f64)
}

fn compare(
    conn: &Connection,
    v: &FeatureVersion,
    cfg: &Config,
    judgements: &[Judgement],
) -> Result<Comparison> {
    let vectors = ranking::embeddings(conn, v)?;
    // Seeds as they are now: the app does not keep their history.
    let seeds: HashSet<String> = crate::discovery::seeds(conn)?
        .iter()
        .map(|s| fold(&s.value))
        .collect();
    let empty = Profile::build(vec![], vec![], cfg);
    let (mut pairs, mut without) = (vec![], 0);
    for j in judgements {
        let (evidence, artist, label): (f64, String, String) = conn.query_row(
            "SELECT COALESCE((SELECT MAX(e.confidence) FROM evidence e JOIN candidate c ON c.id = e.candidate_id
                              WHERE c.track_id = ?1), 0),
                    COALESCE(m.artist, ''), COALESCE(m.label, '')
             FROM track t LEFT JOIN track_meta m ON m.track_id = t.id WHERE t.id = ?1",
            params![j.track],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let embedding = vectors.get(&j.track).cloned();
        if embedding.is_none() {
            without += 1;
        }
        let item = Item {
            id: j.track.clone(),
            embedding,
            evidence: evidence as f32,
            seed_match: seeds.contains(&fold(&artist))
                || (!label.is_empty() && seeds.contains(&fold(&label))),
            artist: fold(&artist),
        };
        let (mut positives, mut dislikes) = (vec![], vec![]);
        for (track, kind) in ratings_before(conn, j.rowid)? {
            // Never the judged track itself.
            if track == j.track {
                continue;
            }
            let Some(e) = vectors.get(&track) else { continue };
            match ranking::stars(&kind) {
                Some(weight) => positives.push(Positive {
                    embedding: e.clone(),
                    weight,
                }),
                None if kind == "thumbs_down" => dislikes.push(e.clone()),
                None => {}
            }
        }
        let profile = Profile::build(positives, dislikes, cfg);
        pairs.push((
            ranking::score(&empty, &item, cfg).score,
            ranking::score(&profile, &item, cfg).score,
            strong(&j.kind),
        ));
    }
    let cultural: Vec<(f32, bool)> = pairs.iter().map(|p| (p.0, p.2)).collect();
    let combined: Vec<(f32, bool)> = pairs.iter().map(|p| (p.1, p.2)).collect();
    Ok(Comparison {
        judgements: pairs.len(),
        strong_positives: pairs.iter().filter(|p| p.2).count(),
        without_embedding: without,
        auc_cultural: auc(&cultural),
        auc_combined: auc(&combined),
        auc_difference_interval: bootstrap_difference(&pairs),
        top_fifth_strong_rate_cultural: top_fifth_rate(&cultural),
        top_fifth_strong_rate_combined: top_fifth_rate(&combined),
    })
}

fn counts(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(p, |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn stability(conn: &Connection, v: &FeatureVersion) -> Result<Stability> {
    let mut stmt = conn.prepare(
        "SELECT track_id, audio_file_id, embedding FROM feature_record
         WHERE model_id = ?1 AND weights_checksum = ?2 AND preprocessing_version = ?3
           AND segment_index IS NULL AND embedding IS NOT NULL AND audio_file_id IS NOT NULL
         ORDER BY track_id, audio_file_id, created_at DESC",
    )?;
    let rows = stmt
        .query_map(
            params![v.model_id, v.weights_checksum, v.preprocessing_version],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // One embedding per copy, grouped by track.
    let mut by_track: HashMap<String, Vec<(String, crate::analysis::UnitEmbedding)>> = HashMap::new();
    for (track, file, bytes) in rows {
        let copies = by_track.entry(track).or_default();
        if copies.iter().all(|(f, _)| *f != file) {
            copies.push((
                file,
                crate::analysis::Embedding::from_bytes(v.clone(), &bytes).unit(),
            ));
        }
    }
    let mut sims = vec![];
    for copies in by_track.values() {
        for i in 0..copies.len() {
            for j in i + 1..copies.len() {
                if let Ok(s) = copies[i].1.cosine(&copies[j].1) {
                    sims.push(s as f64);
                }
            }
        }
    }
    Ok(Stability {
        pairs: sims.len(),
        mean_similarity: (!sims.is_empty()).then(|| sims.iter().sum::<f64>() / sims.len() as f64),
        min_similarity: sims.iter().copied().reduce(f64::min),
    })
}

/// Everything the pilot protocol asks for that this library can answer.
pub fn report(
    conn: &Connection,
    v: &FeatureVersion,
    cfg: &Config,
    ready_threshold: i64,
    app_version: &str,
    now: i64,
) -> Result<Report> {
    let judgements = first_judgements(conn)?;
    let kind_count = |k: &str| judgements.iter().filter(|j| j.kind == k).count();
    let judged_tracks: Vec<&str> = judgements.iter().map(|j| j.track.as_str()).collect();
    let flagged = |table: &str| -> Result<usize> {
        let mut n = 0;
        for t in &judged_tracks {
            let hit: bool = conn.query_row(
                &format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE track_id = ?1)"),
                params![t],
                |r| r.get(0),
            )?;
            n += usize::from(hit);
        }
        Ok(n)
    };
    let judged = judgements.len();
    let strong_n = judgements.iter().filter(|j| strong(&j.kind)).count();
    Ok(Report {
        report_version: REPORT_VERSION,
        generated_at: now,
        app_version: app_version.into(),
        model_id: v.model_id.clone(),
        first_judgement_at: judgements.first().map(|j| j.at),
        last_judgement_at: judgements.last().map(|j| j.at),
        ranking: compare(conn, v, cfg, &judgements)?,
        outcomes: Outcomes {
            judged,
            thumbs_down: kind_count("thumbs_down"),
            one_star: kind_count("star1"),
            two_stars: kind_count("star2"),
            three_stars: kind_count("star3"),
            skipped: kind_count("skip"),
            strong_positive_rate: rate(strong_n, judged),
            keep_rate: rate(flagged("keep_decision")?, judged),
            wrong_version_rate: rate(flagged("version_flag")?, judged),
        },
        buffer: crate::replenish::availability(conn, 0, ready_threshold)?,
        source_runs: counts(
            conn,
            "SELECT outcome, COUNT(*) FROM source_run GROUP BY outcome",
            [],
        )?,
        analysis: counts(
            conn,
            "SELECT state, COUNT(*) FROM analysis_state
             WHERE model_id = ?1 AND weights_checksum = ?2 AND preprocessing_version = ?3 GROUP BY state",
            params![v.model_id, v.weights_checksum, v.preprocessing_version],
        )?,
        embedding_stability: stability(conn, v)?,
        not_recorded: vec![
            "Extraction time and peak memory per track: measured with the analysis benchmark instead.".into(),
            "Queue recovery after a crash: covered by the recovery tests, not counted in use.".into(),
            "Seed history: seeds are taken as they are when the report is made.".into(),
        ],
    })
}

#[cfg(test)]
#[path = "evaluation_tests.rs"]
mod tests;
