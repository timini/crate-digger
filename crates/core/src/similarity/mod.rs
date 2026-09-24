//! The library map (#25): a nearest-neighbour graph, DBSCAN clusters and
//! 2D display coordinates, from whole-track embeddings of one model version.
//!
//! Clusters come from the embeddings themselves (unit vectors, cosine
//! distance), never from the 2D layout. The layout is only for display.
//! Tracks without an embedding of the map's version are listed, not placed.
//! Building reads embeddings and writes only the map tables: playlists,
//! ratings and metadata are never touched.

pub mod compute;

use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::analysis::{Embedding, FeatureVersion};
use crate::util::new_id;
use crate::Result;
use compute::{Cancelled, Control, Matrix};

/// Changes when the algorithms or their defaults change.
pub const PIPELINE_VERSION: &str = "map-1";

/// Library tracks: those with a file imported or archived here.
const IN_LIBRARY: &str =
    "EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = t.id AND f.origin IN ('imported', 'archived'))";

/// An embedding of the version bound to ?2, ?3 and ?4 exists for track t.
const HAS_EMBEDDING: &str = "EXISTS (SELECT 1 FROM feature_record fr WHERE fr.track_id = t.id
    AND fr.model_id = ?2 AND fr.weights_checksum = ?3 AND fr.preprocessing_version = ?4
    AND fr.segment_index IS NULL AND fr.embedding IS NOT NULL)";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// Neighbours per track in the graph and the layout.
    pub neighbours: usize,
    /// DBSCAN min_samples, the track itself included.
    pub min_samples: usize,
    /// DBSCAN eps as a cosine distance. None picks it from the data.
    pub eps: Option<f32>,
    pub epochs: usize,
    pub seed: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            neighbours: 10,
            min_samples: 5,
            eps: None,
            epochs: 200,
            seed: 42,
        }
    }
}

impl Params {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !(2..=50).contains(&self.neighbours) {
            return Err("Neighbours must be between 2 and 50.".into());
        }
        if !(2..=50).contains(&self.min_samples) {
            return Err("Tracks needed to start a cluster must be between 2 and 50.".into());
        }
        if self.eps.is_some_and(|e| !(e > 0.0 && e <= 2.0)) {
            return Err("The cluster distance must be above 0 and at most 2.".into());
        }
        if !(10..=1000).contains(&self.epochs) {
            return Err("Layout epochs must be between 10 and 1000.".into());
        }
        Ok(())
    }
}

/// What a saved map was built from, so it can be reproduced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub pipeline_version: String,
    pub distance: String,
    pub preprocessing: String,
    pub neighbours: usize,
    pub min_samples: usize,
    pub eps: f32,
    pub eps_rule: String,
    pub layout: String,
    pub epochs: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapInfo {
    pub id: String,
    pub version: FeatureVersion,
    pub provenance: Provenance,
    pub placed: usize,
    pub clusters: usize,
    pub noise: usize,
    pub build_ms: i64,
    pub created_at: i64,
}

/// The inputs to a build, read in one query.
pub struct Inputs {
    pub version: FeatureVersion,
    pub ids: Vec<String>,
    pub matrix: Matrix,
    pub newest_feature_at: i64,
}

/// Unit embeddings of `v` for every library track that has one, in track id order.
pub fn load(conn: &Connection, v: &FeatureVersion) -> Result<Inputs> {
    let mut stmt = conn.prepare(&format!(
        "SELECT t.id, fr.embedding, fr.created_at FROM track t
         JOIN feature_record fr ON fr.track_id = t.id
         WHERE {IN_LIBRARY} AND fr.model_id = ?1 AND fr.weights_checksum = ?2
           AND fr.preprocessing_version = ?3 AND fr.segment_index IS NULL AND fr.embedding IS NOT NULL
         ORDER BY t.id, fr.created_at DESC"
    ))?;
    let rows = stmt.query_map(
        params![v.model_id, v.weights_checksum, v.preprocessing_version],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    )?;
    let (mut ids, mut data, mut dims, mut newest) = (vec![], vec![], None, 0);
    for row in rows {
        let (id, bytes, at) = row?;
        if ids.last() == Some(&id) {
            continue; // an older record of the same track
        }
        let unit = Embedding::from_bytes(v.clone(), &bytes).unit();
        let d = *dims.get_or_insert(unit.values().len());
        if unit.values().len() != d || d == 0 {
            continue;
        }
        newest = newest.max(at);
        ids.push(id);
        data.extend_from_slice(unit.values());
    }
    Ok(Inputs {
        version: v.clone(),
        ids,
        matrix: Matrix {
            dims: dims.unwrap_or(0),
            data,
        },
        newest_feature_at: newest,
    })
}

pub struct Built {
    pub provenance: Provenance,
    pub points: Vec<([f32; 2], Option<u32>)>,
    pub neighbours: Vec<Vec<compute::Neighbour>>,
    pub build_ms: i64,
}

/// The numerical part of a build. Needs no database connection.
pub fn compute(inputs: &Inputs, p: &Params, ctl: &Control) -> std::result::Result<Built, Cancelled> {
    let started = Instant::now();
    let m = &inputs.matrix;
    let k = p
        .neighbours
        .max(p.min_samples.saturating_sub(1))
        .min(m.len().saturating_sub(1));
    let knn = if k == 0 {
        vec![vec![]; m.len()]
    } else {
        compute::knn(m, k, ctl)?
    };
    let (eps, eps_rule) = match p.eps {
        Some(e) => (e, "set by the user".to_string()),
        None => (
            compute::auto_eps(&knn, p.min_samples).unwrap_or(0.0),
            format!(
                "median distance to the {} nearest neighbour",
                ordinal(p.min_samples.saturating_sub(1))
            ),
        ),
    };
    let clusters = if !knn.is_empty() && knn.iter().all(|l| l.len() + 1 >= p.min_samples) {
        compute::dbscan(m, &knn, eps, p.min_samples, ctl)?
    } else {
        // Too few tracks for any core point.
        vec![None; m.len()]
    };
    let coords = compute::layout(m, &knn, p.neighbours, p.epochs, p.seed, ctl)?;
    ctl.progress.store(100, std::sync::atomic::Ordering::Relaxed);
    Ok(Built {
        provenance: Provenance {
            pipeline_version: PIPELINE_VERSION.into(),
            distance: "cosine".into(),
            preprocessing: "unit-length whole-track embeddings".into(),
            neighbours: p.neighbours,
            min_samples: p.min_samples,
            eps,
            eps_rule,
            layout: "UMAP objective (min_dist 0.1) from a PCA start".into(),
            epochs: p.epochs,
            seed: p.seed,
        },
        points: coords.into_iter().zip(clusters).collect(),
        neighbours: knn
            .into_iter()
            .map(|mut l| {
                l.truncate(p.neighbours);
                l
            })
            .collect(),
        build_ms: started.elapsed().as_millis() as i64,
    })
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Replace the saved map with a new one.
pub fn save(conn: &Connection, inputs: &Inputs, built: &Built, now: i64) -> Result<MapInfo> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM similarity_map", [])?;
    let id = new_id();
    let clusters = built
        .points
        .iter()
        .filter_map(|p| p.1)
        .max()
        .map(|c| c as usize + 1)
        .unwrap_or(0);
    let noise = built.points.iter().filter(|p| p.1.is_none()).count();
    let v = &inputs.version;
    tx.execute(
        "INSERT INTO similarity_map (id, model_id, weights_checksum, preprocessing_version, pipeline_version,
                                     params, placed, clusters, noise, newest_feature_at, build_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            v.model_id,
            v.weights_checksum,
            v.preprocessing_version,
            PIPELINE_VERSION,
            serde_json::to_string(&built.provenance).expect("serialisable"),
            inputs.ids.len() as i64,
            clusters as i64,
            noise as i64,
            inputs.newest_feature_at,
            built.build_ms,
            now
        ],
    )?;
    {
        let mut point = tx.prepare(
            "INSERT INTO similarity_point (map_id, track_id, x, y, cluster) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        let mut edge = tx.prepare(
            "INSERT INTO similarity_edge (map_id, track_id, neighbour_id, rank, distance)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (i, track) in inputs.ids.iter().enumerate() {
            let ([x, y], cluster) = built.points[i];
            point.execute(params![id, track, x as f64, y as f64, cluster])?;
            for (rank, (j, d)) in built.neighbours[i].iter().enumerate() {
                edge.execute(params![
                    id,
                    track,
                    inputs.ids[*j as usize],
                    rank as i64,
                    *d as f64
                ])?;
            }
        }
    }
    tx.commit()?;
    Ok(current(conn)?.expect("just saved"))
}

pub fn current(conn: &Connection) -> Result<Option<MapInfo>> {
    Ok(conn
        .query_row(
            "SELECT id, model_id, weights_checksum, preprocessing_version, params, placed, clusters, noise,
                    build_ms, created_at
             FROM similarity_map ORDER BY created_at DESC LIMIT 1",
            [],
            |r| {
                Ok(MapInfo {
                    id: r.get(0)?,
                    version: FeatureVersion {
                        model_id: r.get(1)?,
                        weights_checksum: r.get(2)?,
                        preprocessing_version: r.get(3)?,
                    },
                    provenance: serde_json::from_str(&r.get::<_, String>(4)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
                    })?,
                    placed: r.get::<_, i64>(5)? as usize,
                    clusters: r.get::<_, i64>(6)? as usize,
                    noise: r.get::<_, i64>(7)? as usize,
                    build_ms: r.get(8)?,
                    created_at: r.get(9)?,
                })
            },
        )
        .optional()?)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coverage {
    /// Library tracks.
    pub eligible: usize,
    /// Library tracks with an embedding of the current model.
    pub embedded: usize,
    /// Why the saved map no longer matches the library, if it does not.
    pub stale: Option<String>,
}

pub fn coverage(conn: &Connection, v: &FeatureVersion) -> Result<Coverage> {
    let map = current(conn)?;
    let map_id = map.as_ref().map(|m| m.id.clone()).unwrap_or_default();
    let vp = params![map_id, v.model_id, v.weights_checksum, v.preprocessing_version];
    let (eligible, embedded, added, removed, reanalysed): (i64, i64, i64, i64, bool) = conn.query_row(
        &format!(
            "SELECT
                (SELECT COUNT(*) FROM track t WHERE {IN_LIBRARY}),
                (SELECT COUNT(*) FROM track t WHERE {IN_LIBRARY} AND {HAS_EMBEDDING}),
                (SELECT COUNT(*) FROM track t WHERE {IN_LIBRARY} AND {HAS_EMBEDDING}
                    AND t.id NOT IN (SELECT track_id FROM similarity_point WHERE map_id = ?1)),
                (SELECT COUNT(*) FROM similarity_point p JOIN track t ON t.id = p.track_id
                    WHERE p.map_id = ?1 AND NOT ({IN_LIBRARY} AND {HAS_EMBEDDING})),
                EXISTS (SELECT 1 FROM similarity_point p JOIN feature_record fr ON fr.track_id = p.track_id
                    WHERE p.map_id = ?1 AND fr.model_id = ?2 AND fr.weights_checksum = ?3
                      AND fr.preprocessing_version = ?4 AND fr.segment_index IS NULL
                      AND fr.created_at > (SELECT newest_feature_at FROM similarity_map WHERE id = ?1))"
        ),
        vp,
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let stale = match map {
        None => None,
        Some(map) if map.version != *v => Some(
            "The map was made with a different analysis model. Rebuild it to use the current one.".into(),
        ),
        Some(_) => {
            let mut parts = vec![];
            if added > 0 {
                parts.push(format!("{added} newly analysed"));
            }
            if removed > 0 {
                parts.push(format!("{removed} no longer in the library"));
            }
            if reanalysed {
                parts.push("some analysed again".into());
            }
            (!parts.is_empty()).then(|| {
                format!(
                    "Tracks changed since the map was made ({}). Rebuilding includes them; points may move and clusters may be renumbered.",
                    parts.join(", ")
                )
            })
        }
    };
    Ok(Coverage {
        eligible: eligible as usize,
        embedded: embedded as usize,
        stale,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapPoint {
    pub track_id: String,
    pub x: f32,
    pub y: f32,
    pub cluster: Option<u32>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub release_country: Option<String>,
    pub playlists: Vec<String>,
}

/// Every placed track with what the map can be coloured by.
pub fn points(conn: &Connection, map_id: &str) -> Result<Vec<MapPoint>> {
    let mut stmt = conn.prepare(
        "SELECT p.track_id, p.x, p.y, p.cluster, m.artist, m.title, m.mix, m.year, m.genre, m.label,
                (SELECT value FROM field_value fv WHERE fv.track_id = p.track_id AND fv.field = 'release_country'
                 ORDER BY fv.source = 'musicbrainz' DESC, fv.updated_at DESC LIMIT 1),
                (SELECT group_concat(DISTINCT pe.playlist_id) FROM playlist_entry pe WHERE pe.track_id = p.track_id)
         FROM similarity_point p LEFT JOIN track_meta m ON m.track_id = p.track_id
         WHERE p.map_id = ?1 ORDER BY p.track_id",
    )?;
    let rows = stmt.query_map(params![map_id], |r| {
        Ok(MapPoint {
            track_id: r.get(0)?,
            x: r.get::<_, f64>(1)? as f32,
            y: r.get::<_, f64>(2)? as f32,
            cluster: r.get(3)?,
            artist: r.get(4)?,
            title: r.get(5)?,
            mix: r.get(6)?,
            year: r.get(7)?,
            genre: r.get(8)?,
            label: r.get(9)?,
            release_country: r.get(10)?,
            playlists: r
                .get::<_, Option<String>>(11)?
                .map(|s| {
                    let mut ids: Vec<String> = s.split(',').map(str::to_string).collect();
                    ids.sort();
                    ids
                })
                .unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Graph edges up to `max_rank` neighbours per track: (track, neighbour, distance).
pub fn edges(conn: &Connection, map_id: &str, max_rank: usize) -> Result<Vec<(String, String, f32)>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, neighbour_id, distance FROM similarity_edge
         WHERE map_id = ?1 AND rank < ?2 ORDER BY track_id, rank",
    )?;
    let rows = stmt.query_map(params![map_id, max_rank as i64], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_, f64>(2)? as f32))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapNeighbour {
    pub track_id: String,
    pub rank: usize,
    /// Cosine distance between the embeddings.
    pub distance: f32,
    pub similarity: f32,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
}

pub fn neighbours(conn: &Connection, map_id: &str, track_id: &str) -> Result<Vec<MapNeighbour>> {
    let mut stmt = conn.prepare(
        "SELECT e.neighbour_id, e.rank, e.distance, m.artist, m.title, m.mix FROM similarity_edge e
         LEFT JOIN track_meta m ON m.track_id = e.neighbour_id
         WHERE e.map_id = ?1 AND e.track_id = ?2 ORDER BY e.rank",
    )?;
    let rows = stmt.query_map(params![map_id, track_id], |r| {
        let distance = r.get::<_, f64>(2)? as f32;
        Ok(MapNeighbour {
            track_id: r.get(0)?,
            rank: r.get::<_, i64>(1)? as usize,
            distance,
            similarity: 1.0 - distance,
            artist: r.get(3)?,
            title: r.get(4)?,
            mix: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unplaced {
    pub track_id: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub reason: String,
}

/// Library tracks the map cannot place, with the reason.
pub fn unplaced(conn: &Connection, v: &FeatureVersion) -> Result<Vec<Unplaced>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT t.id, m.artist, m.title, s.state, s.reason FROM track t
         LEFT JOIN track_meta m ON m.track_id = t.id
         LEFT JOIN analysis_state s ON s.track_id = t.id AND s.model_id = ?2
              AND s.weights_checksum = ?3 AND s.preprocessing_version = ?4
         WHERE ?1 = ?1 AND {IN_LIBRARY} AND NOT {HAS_EMBEDDING}
         ORDER BY m.artist, m.title, t.id"
    ))?;
    let rows = stmt.query_map(
        params!["", v.model_id, v.weights_checksum, v.preprocessing_version],
        |r| {
            let state: Option<String> = r.get(3)?;
            let reason: Option<String> = r.get(4)?;
            Ok(Unplaced {
                track_id: r.get(0)?,
                artist: r.get(1)?,
                title: r.get(2)?,
                reason: match (state.as_deref(), reason) {
                    (_, Some(reason)) => reason,
                    (Some("failed"), None) => "Analysis failed.".into(),
                    (Some(s), None) if s != "done" => "Waiting for analysis.".into(),
                    _ => "Not analysed with the current model yet.".into(),
                },
            })
        },
    )?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
