//! Storing analysis results and planning (re-)analysis.
//!
//! Features are stored per track with the exact version that produced them
//! and survive deletion of the audio they came from. When the pinned
//! version changes, re-analysis is queued only where playable audio exists;
//! everything else is marked "needs audio" once, visibly, and not retried
//! until audio comes back.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::protocol::{Analysis, FingerprintOut};
use super::{Embedding, FeatureVersion};
use crate::domain::Field;
use crate::jobs::{kinds, NewJob};
use crate::util::{new_id, now_ms};
use crate::{library, meta, Result};

fn fingerprint_id(fp: &FingerprintOut) -> String {
    let bytes: Vec<u8> = fp.data.iter().flat_map(|v| v.to_le_bytes()).collect();
    format!("{}:{}", fp.algorithm, &blake3::hash(&bytes).to_hex()[..16])
}

/// Save a finished analysis of `file_id` (a copy of `track_id`).
pub fn store(conn: &Connection, track_id: &str, file_id: &str, a: &Analysis) -> Result<()> {
    let now = now_ms();
    let source_fp = fingerprint_id(&a.fingerprint);
    let tx = conn.unchecked_transaction()?;

    let fp_bytes: Vec<u8> = a.fingerprint.data.iter().flat_map(|v| v.to_le_bytes()).collect();
    let fp_id = new_id();
    tx.execute(
        "INSERT INTO fingerprint (id, track_id, audio_file_id, algorithm, duration_ms, data, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (audio_file_id, algorithm) DO UPDATE SET
             track_id = excluded.track_id, duration_ms = excluded.duration_ms, data = excluded.data,
             created_at = excluded.created_at",
        params![
            fp_id,
            track_id,
            file_id,
            a.fingerprint.algorithm,
            a.fingerprint.duration_ms as i64,
            fp_bytes,
            now
        ],
    )?;
    let stored_id: String = tx.query_row(
        "SELECT id FROM fingerprint WHERE audio_file_id = ?1 AND algorithm = ?2",
        params![file_id, a.fingerprint.algorithm],
        |r| r.get(0),
    )?;
    index_fingerprint(&tx, &stored_id, &fp_bytes)?;

    let mut versions: Vec<FeatureVersion> = Vec::new();
    for e in &a.embeddings {
        if !versions.contains(&e.version) {
            versions.push(e.version.clone());
        }
    }
    for v in &versions {
        // Re-analysing the same file at the same version replaces its rows.
        tx.execute(
            "DELETE FROM feature_record WHERE audio_file_id = ?1 AND model_id = ?2
               AND weights_checksum = ?3 AND preprocessing_version = ?4",
            params![file_id, v.model_id, v.weights_checksum, v.preprocessing_version],
        )?;
    }
    let details = serde_json::json!({
        "tempo_confidence": a.tempo_confidence,
        "key": a.key,
        "quality": a.quality,
        "stats": a.stats,
    });
    for e in &a.embeddings {
        let (start, end) = match e.segment.and_then(|i| a.segments.get(i)) {
            Some(s) => (s.start_ms as i64, s.end_ms as i64),
            None => (0, a.duration_ms as i64),
        };
        let bytes: Vec<u8> = e.vector.iter().flat_map(|v| v.to_le_bytes()).collect();
        let summary = e.segment.is_none();
        tx.execute(
            "INSERT INTO feature_record (id, track_id, audio_file_id, model_id, weights_checksum,
                 preprocessing_version, source_fingerprint, segment_start_ms, segment_end_ms, dims, embedding,
                 tempo, musical_key, loudness_lufs, quality, segment_index, details, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                new_id(),
                track_id,
                file_id,
                e.version.model_id,
                e.version.weights_checksum,
                e.version.preprocessing_version,
                source_fp,
                start,
                end,
                e.vector.len() as i64,
                bytes,
                summary.then_some(a.tempo_bpm).flatten(),
                summary
                    .then(|| a.key.as_ref().map(|k| k.camelot.clone()))
                    .flatten(),
                summary.then_some(a.loudness_lufs).flatten(),
                summary.then_some(1.0 - a.quality.clipping_ratio as f64),
                e.segment.map(|i| i as i64),
                summary.then(|| details.to_string()),
                now
            ],
        )?;
    }
    for v in &versions {
        set_state_tx(&tx, track_id, v, "done", None, now)?;
    }

    tx.execute(
        "UPDATE audio_file SET duration_ms = ?2, waveform = COALESCE(?3, waveform) WHERE id = ?1",
        params![
            file_id,
            a.duration_ms as i64,
            (!a.waveform.is_empty()).then_some(&a.waveform)
        ],
    )?;
    // Tempo and key from the main copy fill in metadata the tags lack; tag
    // values (usually from DJ software) and user edits still win.
    let main_copy: bool = tx.query_row(
        "SELECT is_primary = 1 AND variant IS NULL FROM audio_file WHERE id = ?1",
        params![file_id],
        |r| r.get(0),
    )?;
    if main_copy {
        meta::set_extracted(
            &tx,
            track_id,
            "analysis",
            &[
                (Field::Tempo, a.tempo_bpm.map(|t| format!("{t:.1}"))),
                (Field::MusicalKey, a.key.as_ref().map(|k| k.camelot.clone())),
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Every third fingerprint value, as lookup keys. Re-encodes of the same
/// audio share a good fraction of exact values; unrelated audio shares
/// almost none.
pub fn index_fingerprint(conn: &Connection, fingerprint_id: &str, data: &[u8]) -> Result<()> {
    conn.execute(
        "DELETE FROM fingerprint_key WHERE fingerprint_id = ?1",
        params![fingerprint_id],
    )?;
    let mut stmt =
        conn.prepare_cached("INSERT INTO fingerprint_key (key, fingerprint_id) VALUES (?1, ?2)")?;
    for chunk in data.as_chunks::<4>().0.iter().step_by(3) {
        let v = u32::from_le_bytes(*chunk);
        if v != 0 {
            stmt.execute(params![v as i64, fingerprint_id])?;
        }
    }
    Ok(())
}

fn set_state_tx(
    conn: &Connection,
    track_id: &str,
    v: &FeatureVersion,
    state: &str,
    reason: Option<&str>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO analysis_state (track_id, model_id, weights_checksum, preprocessing_version, state, reason,
                                     updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (track_id, model_id, weights_checksum, preprocessing_version)
         DO UPDATE SET state = excluded.state, reason = excluded.reason, updated_at = excluded.updated_at",
        params![track_id, v.model_id, v.weights_checksum, v.preprocessing_version, state, reason, now],
    )?;
    Ok(())
}

pub fn set_state(
    conn: &Connection,
    track_id: &str,
    v: &FeatureVersion,
    state: &str,
    reason: Option<&str>,
) -> Result<()> {
    set_state_tx(conn, track_id, v, state, reason, now_ms())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisStatus {
    pub state: String,
    pub reason: Option<String>,
}

pub fn status(conn: &Connection, track_id: &str, v: &FeatureVersion) -> Result<Option<AnalysisStatus>> {
    Ok(conn
        .query_row(
            "SELECT state, reason FROM analysis_state
             WHERE track_id = ?1 AND model_id = ?2 AND weights_checksum = ?3 AND preprocessing_version = ?4",
            params![track_id, v.model_id, v.weights_checksum, v.preprocessing_version],
            |r| {
                Ok(AnalysisStatus {
                    state: r.get(0)?,
                    reason: r.get(1)?,
                })
            },
        )
        .optional()?)
}

/// The whole-track embedding of `track_id` at version `v`.
pub fn summary_embedding(conn: &Connection, track_id: &str, v: &FeatureVersion) -> Result<Option<Embedding>> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM feature_record
             WHERE track_id = ?1 AND model_id = ?2 AND weights_checksum = ?3 AND preprocessing_version = ?4
               AND segment_index IS NULL AND embedding IS NOT NULL
             ORDER BY created_at DESC LIMIT 1",
            params![track_id, v.model_id, v.weights_checksum, v.preprocessing_version],
            |r| r.get(0),
        )
        .optional()?;
    Ok(bytes.map(|b| Embedding::from_bytes(v.clone(), &b)))
}

/// A stored fingerprint: (algorithm, values, duration in ms).
pub type StoredFingerprint = (String, Vec<u32>, i64);

fn decode_fp(algorithm: String, data: Vec<u8>, duration: i64) -> StoredFingerprint {
    let values = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    (algorithm, values, duration)
}

pub fn fingerprint_of_file(conn: &Connection, file_id: &str) -> Result<Option<StoredFingerprint>> {
    Ok(conn
        .query_row(
            "SELECT algorithm, data, duration_ms FROM fingerprint WHERE audio_file_id = ?1",
            params![file_id],
            |r| Ok(decode_fp(r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// Fingerprints of the unpitched copies of a track.
pub fn fingerprints_of_track(conn: &Connection, track_id: &str) -> Result<Vec<StoredFingerprint>> {
    let mut stmt = conn.prepare(
        "SELECT f.algorithm, f.data, f.duration_ms FROM fingerprint f
         LEFT JOIN audio_file a ON a.id = f.audio_file_id
         WHERE f.track_id = ?1 AND (a.variant IS NULL)",
    )?;
    let rows = stmt
        .query_map(params![track_id], |r| {
            Ok(decode_fp(r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn version_key(v: &FeatureVersion) -> String {
    format!(
        "{}:{}:{}",
        v.model_id, v.preprocessing_version, v.weights_checksum
    )
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Plan {
    pub queued: usize,
    pub needs_audio: usize,
}

/// Queue analysis at version `v` for every track that lacks it and has
/// playable audio (library tracks, and candidates that are already ready or
/// reviewed; the pipeline analyses the rest itself). Tracks that were
/// analysed at another version but have no audio any more are marked
/// "needs audio" instead. Safe to call repeatedly: it never queues twice
/// and never loops on missing audio.
pub fn plan(conn: &Connection, v: &FeatureVersion, now: i64) -> Result<Plan> {
    let mut plan = Plan::default();
    let tracks: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT t.id FROM track t
             WHERE NOT EXISTS (
                     SELECT 1 FROM analysis_state s WHERE s.track_id = t.id AND s.model_id = ?1
                       AND s.weights_checksum = ?2 AND s.preprocessing_version = ?3
                       AND s.state IN ('done', 'queued', 'failed'))
               AND NOT EXISTS (
                     SELECT 1 FROM candidate c WHERE c.track_id = t.id AND c.stage NOT IN ('ready', 'reviewed'))
               AND (EXISTS (SELECT 1 FROM audio_file f WHERE f.track_id = t.id)
                    OR EXISTS (SELECT 1 FROM feature_record r WHERE r.track_id = t.id))",
        )?;
        let rows = stmt
            .query_map(
                params![v.model_id, v.weights_checksum, v.preprocessing_version],
                |r| r.get(0),
            )?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        rows
    };
    for track in tracks {
        match library::playable_file(conn, &track)? {
            Some(f) => {
                crate::jobs::enqueue(
                    conn,
                    &NewJob::new(
                        kinds::ANALYSE,
                        format!("analyse:{}:{}", f.id, version_key(v)),
                        serde_json::json!({ "file_id": f.id, "candidate_id": null }),
                    ),
                    now,
                )?;
                set_state_tx(conn, &track, v, "queued", None, now)?;
                plan.queued += 1;
            }
            None => {
                let current: Option<String> = status(conn, &track, v)?.map(|s| s.state);
                if current.as_deref() != Some("needs_audio") {
                    set_state_tx(
                        conn,
                        &track,
                        v,
                        "needs_audio",
                        Some(&format!(
                            "Analysis with {} needs this track's audio, which is not available. Its earlier \
                             analysis is kept. Relink or fetch the audio to update it.",
                            v.model_id
                        )),
                        now,
                    )?;
                }
                plan.needs_audio += 1;
            }
        }
    }
    Ok(plan)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisSummary {
    pub model: String,
    pub state: Option<AnalysisStatus>,
    pub tempo: Option<f64>,
    pub tempo_confidence: Option<f64>,
    pub key: Option<serde_json::Value>,
    pub loudness_lufs: Option<f64>,
    pub quality: Option<serde_json::Value>,
}

/// What the latest analysis at version `v` found, for display.
pub fn summary(conn: &Connection, track_id: &str, v: &FeatureVersion) -> Result<AnalysisSummary> {
    let row: Option<(Option<f64>, Option<f64>, Option<String>)> = conn
        .query_row(
            "SELECT tempo, loudness_lufs, details FROM feature_record
             WHERE track_id = ?1 AND model_id = ?2 AND weights_checksum = ?3 AND preprocessing_version = ?4
               AND segment_index IS NULL ORDER BY created_at DESC LIMIT 1",
            params![track_id, v.model_id, v.weights_checksum, v.preprocessing_version],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let details: Option<serde_json::Value> = row
        .as_ref()
        .and_then(|r| r.2.as_deref())
        .and_then(|d| serde_json::from_str(d).ok());
    Ok(AnalysisSummary {
        model: v.model_id.clone(),
        state: status(conn, track_id, v)?,
        tempo: row.as_ref().and_then(|r| r.0),
        tempo_confidence: details.as_ref().and_then(|d| d["tempo_confidence"].as_f64()),
        key: details
            .as_ref()
            .map(|d| d["key"].clone())
            .filter(|k| !k.is_null()),
        loudness_lufs: row.as_ref().and_then(|r| r.1),
        quality: details.as_ref().map(|d| d["quality"].clone()),
    })
}
