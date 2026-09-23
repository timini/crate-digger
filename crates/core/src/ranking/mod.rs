//! Tier 2: personal ranking of candidates.
//!
//! Taste is the share of a candidate's nearest rated tracks that the
//! listener liked, times how close its nearest likes are. Liked tracks are
//! also grouped into taste clusters, and the queue avoids repeating one
//! cluster, so distinct tastes are not averaged into one. A dislike lowers
//! only candidates closer to it than to any like; nothing is excluded by
//! artist or label. Cultural evidence counts throughout and carries the
//! ranking until five positive ratings exist. One queue position in five
//! goes to culturally supported candidates outside the known tastes.
//! Weights are chosen in `tests/ranking_calibration.rs`; see
//! docs/ranking-calibration.md.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::analysis::{Embedding, FeatureVersion, UnitEmbedding};
use crate::identity::normalize::fold;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub w_taste: f32,
    pub w_dislike: f32,
    pub w_evidence: f32,
    /// A positive joins a cluster when its similarity to the cluster's
    /// centre is at least this.
    pub cluster_threshold: f32,
    /// Closeness is the mean similarity to this many closest likes.
    pub top_k: usize,
    /// The like rate is taken over this many closest rated tracks.
    pub neighbours: usize,
    /// Similarity to a dislike below this carries no penalty.
    pub dislike_floor: f32,
    /// Score cost per recent queue pick from the same taste cluster.
    pub cluster_repeat_cost: f32,
    /// Every nth queue position is an exploration pick.
    pub exploration_every: usize,
    /// Candidates with taste below this and enough evidence can be exploration picks.
    pub exploration_taste_max: f32,
    pub exploration_evidence_min: f32,
    /// Positives needed before taste fully counts.
    pub cold_start_positives: usize,
}

/// The configuration chosen by the calibration (see docs/ranking-calibration.md).
pub const DEFAULT: Config = Config {
    w_taste: 0.6,
    w_dislike: 0.8,
    w_evidence: 0.5,
    cluster_threshold: 0.3,
    top_k: 3,
    neighbours: 5,
    dislike_floor: 0.3,
    cluster_repeat_cost: 0.15,
    exploration_every: 5,
    exploration_taste_max: 0.5,
    exploration_evidence_min: 0.5,
    cold_start_positives: 5,
};

pub struct Positive {
    pub embedding: UnitEmbedding,
    /// 0.6, 0.8 or 1.0 for one, two or three stars.
    pub weight: f32,
}

pub struct Profile {
    pub clusters: Vec<Vec<Positive>>,
    pub dislikes: Vec<UnitEmbedding>,
    pub positives: usize,
}

impl Profile {
    /// Leader clustering in rating order: each positive joins the cluster
    /// whose centre is most similar, if at least `cluster_threshold`, or
    /// starts a new one. Comparing with centres rather than single members
    /// keeps unrelated styles from chaining together.
    pub fn build(positives: Vec<Positive>, dislikes: Vec<UnitEmbedding>, cfg: &Config) -> Profile {
        let count = positives.len();
        let mut clusters: Vec<(UnitEmbedding, Vec<Positive>)> = vec![];
        for p in positives {
            let best = clusters
                .iter()
                .enumerate()
                .filter_map(|(i, (centre, _))| centre.cosine(&p.embedding).ok().map(|s| (i, s)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, s)) if s >= cfg.cluster_threshold => {
                    clusters[i].1.push(p);
                    let members: Vec<&UnitEmbedding> = clusters[i].1.iter().map(|m| &m.embedding).collect();
                    if let Some(c) = UnitEmbedding::centroid(&members) {
                        clusters[i].0 = c;
                    }
                }
                _ => clusters.push((p.embedding.clone(), vec![p])),
            }
        }
        Profile {
            clusters: clusters.into_iter().map(|(_, m)| m).collect(),
            dislikes,
            positives: count,
        }
    }
}

pub struct Item {
    pub id: String,
    pub embedding: Option<UnitEmbedding>,
    /// Strongest cultural evidence, 0 to 1.
    pub evidence: f32,
    /// Artist or label is one of the user's seeds.
    pub seed_match: bool,
    pub artist: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scored {
    pub id: String,
    pub score: f32,
    pub taste: f32,
    pub dislike: f32,
    pub cluster: Option<usize>,
    pub evidence: f32,
    pub artist: String,
    pub has_embedding: bool,
}

pub fn score(profile: &Profile, item: &Item, cfg: &Config) -> Scored {
    let (mut taste, mut cluster, mut dislike) = (0.0f32, None, 0.0f32);
    if let Some(e) = &item.embedding {
        // (similarity, liked, weight) for every rated track of this version.
        let mut rated: Vec<(f32, bool, f32)> = vec![];
        let mut nearest_like = (f32::MIN, None);
        for (i, members) in profile.clusters.iter().enumerate() {
            for m in members {
                if let Ok(s) = m.embedding.cosine(e) {
                    rated.push((s, true, m.weight));
                    if s > nearest_like.0 {
                        nearest_like = (s, Some(i));
                    }
                }
            }
        }
        let mut nearest_dislike = f32::MIN;
        for d in &profile.dislikes {
            if let Ok(s) = d.cosine(e) {
                rated.push((s, false, 1.0));
                nearest_dislike = nearest_dislike.max(s);
            }
        }
        cluster = nearest_like.1;
        if !rated.is_empty() {
            rated.sort_by(|a, b| b.0.total_cmp(&a.0));
            // How much of the neighbourhood the listener liked...
            let near = &rated[..cfg.neighbours.min(rated.len()).max(1)];
            let (liked, total) = near.iter().fold((0.0f32, 0.0f32), |(l, t), (s, is_like, w)| {
                let s = s.max(0.0);
                (if *is_like { l + s * w } else { l }, t + s)
            });
            let rate = if total > 0.0 { liked / total } else { 0.0 };
            // ...times how close the nearest likes are.
            let likes: Vec<f32> = rated.iter().filter(|r| r.1).map(|r| r.0 * r.2).collect();
            let k = cfg.top_k.min(likes.len());
            let closeness = if k == 0 {
                0.0
            } else {
                likes[..k].iter().sum::<f32>() / k as f32
            };
            taste = rate * closeness.max(0.0);
        }
        // A dislike closer than every like suppresses copies and near copies
        // of that version without writing off a style the listener likes.
        let like = nearest_like.0.max(0.0);
        if nearest_dislike > cfg.dislike_floor && nearest_dislike > like {
            dislike = (nearest_dislike - like.max(cfg.dislike_floor)) / (1.0 - cfg.dislike_floor);
        }
    }
    let taste = taste.max(0.0);
    let warm = cfg.w_taste * taste - cfg.w_dislike * dislike + cfg.w_evidence * item.evidence;
    let cold = 0.7 * item.evidence + 0.3 * f32::from(u8::from(item.seed_match)) - cfg.w_dislike * dislike;
    let alpha = (profile.positives as f32 / cfg.cold_start_positives.max(1) as f32).min(1.0);
    Scored {
        id: item.id.clone(),
        score: alpha * warm + (1.0 - alpha) * cold,
        taste,
        dislike,
        cluster,
        evidence: item.evidence,
        artist: item.artist.clone(),
        has_embedding: item.embedding.is_some(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Personal,
    Exploration,
}

/// Queue order. Every `exploration_every`th position is filled from
/// culturally supported candidates outside the known clusters; the rest
/// by score, avoiding repeats of the same artist and cluster.
pub fn order(scored: Vec<Scored>, profile: &Profile, cfg: &Config) -> Vec<(Scored, Slot)> {
    let warm = profile.positives >= cfg.cold_start_positives;
    let (mut explore, mut main): (Vec<Scored>, Vec<Scored>) = scored.into_iter().partition(|s| {
        warm && s.has_embedding
            && s.taste < cfg.exploration_taste_max
            && s.evidence >= cfg.exploration_evidence_min
    });
    explore.sort_by(|a, b| {
        b.evidence
            .total_cmp(&a.evidence)
            .then(b.score.total_cmp(&a.score))
    });
    main.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut explore = std::collections::VecDeque::from(explore);
    let total = main.len() + explore.len();
    let mut out: Vec<(Scored, Slot)> = Vec::with_capacity(total);
    let every = cfg.exploration_every.max(2);
    while out.len() < total {
        let position = out.len() + 1;
        if (position.is_multiple_of(every) || main.is_empty()) && !explore.is_empty() {
            out.push((explore.pop_front().unwrap(), Slot::Exploration));
            continue;
        }
        // Diversity: prefer a different artist from the last three picks and
        // clusters not heard in the last four, when the score cost is small.
        let recent: Vec<&str> = out.iter().rev().take(3).map(|(s, _)| s.artist.as_str()).collect();
        let recent_clusters: Vec<usize> = out.iter().rev().take(4).filter_map(|(s, _)| s.cluster).collect();
        let pick = main
            .iter()
            .take(50)
            .enumerate()
            .map(|(i, s)| {
                let mut e = s.score;
                if !s.artist.is_empty() && recent.contains(&s.artist.as_str()) {
                    e -= 0.15;
                }
                if let Some(c) = s.cluster {
                    e -= cfg.cluster_repeat_cost * recent_clusters.iter().filter(|r| **r == c).count() as f32;
                }
                (i, e)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
            .unwrap_or(0);
        out.push((main.remove(pick), Slot::Personal));
    }
    out
}

fn stars(kind: &str) -> Option<f32> {
    match kind {
        "star1" => Some(0.6),
        "star2" => Some(0.8),
        "star3" => Some(1.0),
        _ => None,
    }
}

/// All summary embeddings of one version, by track.
fn embeddings(conn: &Connection, v: &FeatureVersion) -> Result<HashMap<String, UnitEmbedding>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, embedding FROM feature_record
         WHERE model_id = ?1 AND weights_checksum = ?2 AND preprocessing_version = ?3
           AND segment_index IS NULL AND embedding IS NOT NULL
         ORDER BY created_at",
    )?;
    let rows = stmt.query_map(
        params![v.model_id, v.weights_checksum, v.preprocessing_version],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
    )?;
    let mut out = HashMap::new();
    for row in rows {
        let (track, bytes) = row?;
        out.insert(track, Embedding::from_bytes(v.clone(), &bytes).unit());
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct RerankSummary {
    pub candidates: usize,
    pub queued: usize,
    pub positives: usize,
    pub clusters: usize,
}

/// Recompute scores and the review order from current ratings. Only
/// embeddings of `v` are used, so versions are never mixed.
pub fn rerank(conn: &Connection, v: &FeatureVersion, cfg: &Config) -> Result<RerankSummary> {
    let vectors = embeddings(conn, v)?;
    let (mut positives, mut dislikes) = (vec![], vec![]);
    {
        let mut stmt =
            conn.prepare("SELECT track_id, kind FROM effective_rating ORDER BY created_at DESC LIMIT 500")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (track, kind) in rows.into_iter().rev() {
            let Some(e) = vectors.get(&track) else { continue };
            match stars(&kind) {
                Some(weight) => positives.push(Positive {
                    embedding: e.clone(),
                    weight,
                }),
                None if kind == "thumbs_down" => dislikes.push(e.clone()),
                None => {}
            }
        }
    }
    let seeds: HashSet<String> = crate::discovery::seeds(conn)?
        .iter()
        .map(|s| fold(&s.value))
        .collect();
    let profile = Profile::build(positives, dislikes, cfg);
    let items: Vec<(Item, bool)> = {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.track_id, c.stage = 'ready',
                    COALESCE((SELECT MAX(confidence) FROM evidence e WHERE e.candidate_id = c.id), 0),
                    COALESCE(m.artist, ''), COALESCE(m.label, '')
             FROM candidate c LEFT JOIN track_meta m ON m.track_id = c.track_id
             WHERE c.status = 'active'
               AND c.stage IN ('identified', 'acquisition_queued', 'downloading', 'validating', 'analysing', 'ready')",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, track, ready, evidence, artist, label)| {
                let seed_match =
                    seeds.contains(&fold(&artist)) || (!label.is_empty() && seeds.contains(&fold(&label)));
                (
                    Item {
                        id,
                        embedding: vectors.get(&track).cloned(),
                        evidence: evidence as f32,
                        seed_match,
                        artist: fold(&artist),
                    },
                    ready,
                )
            })
            .collect()
    };
    let mut ready = vec![];
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE candidate SET queue_rank = NULL, rank_note = NULL WHERE queue_rank IS NOT NULL",
        [],
    )?;
    for (item, is_ready) in &items {
        let s = score(&profile, item, cfg);
        tx.execute(
            "UPDATE candidate SET score = ?2 WHERE id = ?1",
            params![s.id, s.score as f64],
        )?;
        if *is_ready {
            ready.push(s);
        }
    }
    let queued = ready.len();
    for (position, (s, slot)) in order(ready, &profile, cfg).into_iter().enumerate() {
        let note = match slot {
            Slot::Exploration => Some("Exploration: supported by sources, outside your usual sound"),
            Slot::Personal => None,
        };
        tx.execute(
            "UPDATE candidate SET queue_rank = ?2, rank_note = ?3 WHERE id = ?1",
            params![s.id, position as i64, note],
        )?;
    }
    tx.commit()?;
    Ok(RerankSummary {
        candidates: items.len(),
        queued,
        positives: profile.positives,
        clusters: profile.clusters.len(),
    })
}

#[cfg(test)]
mod tests;
