//! Sharing with the central catalogue, and reusing what others shared.
//!
//! Only recordings identified with confidence (a MusicBrainz recording,
//! ISRC or Discogs id) are shared, so the catalogue never merges tracks on
//! a guess. A contribution carries metadata, the preferred YouTube link and
//! the pinned model's embedding; its types have no field for paths, ratings
//! or credentials. Contributions wait in a durable outbox until the service
//! acknowledges them. Sharing is off by default and turning it off holds
//! anything unsent.

use std::sync::Arc;

use cd_protocol::{
    Contribution, ExternalId, FeatureVersion, Features, Metadata, RecordingKey, Reference, Validate,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::adapters::{AdapterError, CentralLookup, CentralSync};
use crate::analysis::store::summary_embedding;
use crate::jobs::worker::{Handler, JobCtx};
use crate::jobs::{kinds, JobError, NewJob};
use crate::util::new_id;
use crate::{library, meta, settings, Result};

pub const CONNECTOR: &str = "central";
/// Contributors who must agree before a shared embedding is used here.
pub const MIN_AGREEMENT: u32 = 2;
const SHAREABLE_IDS: [&str; 4] = [
    "musicbrainz_recording",
    "isrc",
    "discogs_release",
    "discogs_track",
];

pub fn enabled(conn: &Connection) -> Result<bool> {
    settings::get_or(conn, settings::keys::SHARING, false)
}

/// The pinned analysis version, the only one shared.
pub fn shared_version() -> crate::analysis::FeatureVersion {
    let (v, _) = cd_protocol::SHARED_FEATURE_VERSIONS[0];
    crate::analysis::FeatureVersion {
        model_id: v.model_id.into(),
        weights_checksum: v.weights_checksum.into(),
        preprocessing_version: v.preprocessing_version.into(),
    }
}

pub fn recording_key(conn: &Connection, track_id: &str) -> Result<Option<RecordingKey>> {
    let mut stmt = conn.prepare(
        "SELECT namespace, value FROM track_external_id WHERE track_id = ?1 ORDER BY namespace, value",
    )?;
    let ids: Vec<ExternalId> = stmt
        .query_map(params![track_id], |r| {
            Ok(ExternalId {
                source: r.get(0)?,
                id: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|e| SHAREABLE_IDS.contains(&e.source.as_str()))
        .take(cd_protocol::MAX_EXTERNAL_IDS)
        .collect();
    Ok((!ids.is_empty()).then_some(RecordingKey {
        fingerprint_hash: None,
        external_ids: ids,
    }))
}

fn features(conn: &Connection, track_id: &str) -> Result<Option<Features>> {
    let v = shared_version();
    let Some(embedding) = summary_embedding(conn, track_id, &v)? else {
        return Ok(None);
    };
    let (tempo, key, loudness): (Option<f64>, Option<String>, Option<f64>) = conn
        .query_row(
            "SELECT tempo, musical_key, loudness_lufs FROM feature_record
             WHERE track_id = ?1 AND model_id = ?2 AND segment_index IS NULL
             ORDER BY created_at DESC LIMIT 1",
            params![track_id, v.model_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .unwrap_or_default();
    let bytes = embedding.to_bytes();
    Ok(Some(Features {
        version: FeatureVersion {
            model_id: v.model_id,
            weights_checksum: v.weights_checksum,
            preprocessing_version: v.preprocessing_version,
        },
        embedding: bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        tempo_bpm: tempo.map(|t| t as f32),
        key_camelot: key,
        loudness_lufs: loudness.map(|l| l as f32),
    }))
}

/// What would be shared about a track, or nothing if it is not identified.
pub fn contribution(conn: &Connection, track_id: &str) -> Result<Option<Contribution>> {
    let Some(recording) = recording_key(conn, track_id)? else {
        return Ok(None);
    };
    let m = meta::effective(conn, track_id)?;
    let duration = library::playable_file(conn, track_id)?.and_then(|f| f.duration_ms);
    let references: Vec<Reference> = conn
        .query_row(
            "SELECT url FROM youtube_match WHERE track_id = ?1 AND preferred = 1 AND rejected = 0",
            params![track_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .into_iter()
        .map(|url| Reference {
            kind: "youtube".into(),
            url,
        })
        .collect();
    let mut c = Contribution {
        idempotency_key: String::new(),
        recording,
        metadata: Some(Metadata {
            artist: m.artist,
            title: m.title,
            mix: m.mix,
            label: m.label,
            release: m.release,
            year: m.year.map(|y| y as i32),
            duration_ms: duration.map(|d| d.max(0) as u64),
        }),
        features: features(conn, track_id)?,
        references,
        correction: false,
    };
    // The same content gives the same key, so it is only ever sent once.
    let body = serde_json::to_string(&c)?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in body.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    c.idempotency_key = format!(
        "t{}-{h:016x}",
        &track_id.replace('-', "")[..12.min(track_id.len())]
    );
    Ok(c.validate().is_ok().then_some(c))
}

/// Put a track's contribution in the outbox, if sharing is on and it changed.
pub fn queue_share(conn: &Connection, track_id: &str, now: i64) -> Result<bool> {
    if !enabled(conn)? {
        return Ok(false);
    }
    let Some(c) = contribution(conn, track_id)? else {
        return Ok(false);
    };
    let id = new_id();
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO sync_outbox (id, idempotency_key, kind, payload, state, created_at)
         VALUES (?1, ?2, 'contribution', ?3, 'pending', ?4)",
        params![id, c.idempotency_key, serde_json::to_string(&c)?, now],
    )?;
    if inserted == 0 {
        return Ok(false);
    }
    crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::SYNC,
            format!("sync:{id}"),
            serde_json::json!({ "outbox_id": id }),
        )
        .connector(CONNECTOR),
        now,
    )?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboxSummary {
    pub pending: i64,
    pub acked: i64,
    pub rejected: i64,
}

pub fn outbox(conn: &Connection) -> Result<OutboxSummary> {
    let n = |state: &str| -> Result<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE state = ?1",
            params![state],
            |r| r.get(0),
        )?)
    };
    Ok(OutboxSummary {
        pending: n("pending")?,
        acked: n("acked")?,
        rejected: n("rejected")?,
    })
}

#[derive(Serialize, Deserialize)]
struct SyncPayload {
    outbox_id: String,
}

/// Sends one outbox item. Outages and rate limits wait without using up
/// attempts, so an unacknowledged item is never dropped.
pub struct ShareHandler {
    pub central: Arc<dyn CentralSync>,
}

const RETRY_LATER_MS: i64 = 10 * 60_000;

impl Handler for ShareHandler {
    fn kind(&self) -> &'static str {
        kinds::SYNC
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let SyncPayload { outbox_id } = ctx.payload()?;
        let row: Option<(String, String, String)> = ctx
            .conn
            .query_row(
                "SELECT idempotency_key, payload, state FROM sync_outbox WHERE id = ?1",
                params![outbox_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(crate::Error::from)?;
        let Some((key, payload, state)) = row else {
            return Ok(());
        };
        if state != "pending" {
            return Ok(());
        }
        if !enabled(ctx.conn)? {
            return Err(JobError::Wait {
                reason: "Sharing is off; this contribution waits until it is turned on.".into(),
                delay_ms: 60 * 60_000,
            });
        }
        let payload: serde_json::Value = serde_json::from_str(&payload).map_err(crate::Error::from)?;
        ctx.conn
            .execute(
                "UPDATE sync_outbox SET attempts = attempts + 1 WHERE id = ?1",
                params![outbox_id],
            )
            .map_err(crate::Error::from)?;
        match self.central.submit(&key, "contribution", &payload) {
            Ok(_) => {
                ctx.conn
                    .execute(
                        "UPDATE sync_outbox SET state = 'acked', acked_at = ?2, last_error = NULL WHERE id = ?1",
                        params![outbox_id, ctx.now()],
                    )
                    .map_err(crate::Error::from)?;
                Ok(())
            }
            Err(AdapterError::Invalid(reason)) => {
                ctx.conn
                    .execute(
                        "UPDATE sync_outbox SET state = 'rejected', last_error = ?2 WHERE id = ?1",
                        params![outbox_id, reason],
                    )
                    .map_err(crate::Error::from)?;
                Ok(())
            }
            Err(e @ AdapterError::Auth(_)) => Err(ctx.adapter_error(e)),
            Err(e) => {
                ctx.conn
                    .execute(
                        "UPDATE sync_outbox SET last_error = ?2 WHERE id = ?1",
                        params![outbox_id, e.to_string()],
                    )
                    .map_err(crate::Error::from)?;
                Err(JobError::Wait {
                    reason: format!("The catalogue is not reachable ({e}); retrying later."),
                    delay_ms: RETRY_LATER_MS,
                })
            }
        }
    }
}

/// Queue both catalogue jobs for a newly identified track.
pub fn after_identified(conn: &Connection, track_id: &str, now: i64) -> Result<()> {
    queue_share(conn, track_id, now)?;
    crate::jobs::enqueue(
        conn,
        &NewJob::new(
            kinds::FEATURE_REUSE,
            format!("reuse:{track_id}"),
            serde_json::json!({ "track_id": track_id }),
        )
        .connector(CONNECTOR),
        now,
    )?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct ReusePayload {
    track_id: String,
}

/// Takes the pinned model's embedding from the catalogue when this track is
/// identified with confidence, has no such embedding yet, and enough
/// contributors agree. Local decoding and validation still happen as usual.
pub struct ReuseHandler {
    /// `None` when the app is not signed in to the catalogue.
    pub central: Option<Arc<dyn CentralLookup>>,
}

pub fn store_shared_features(
    conn: &Connection,
    track_id: &str,
    recording_id: &str,
    f: &Features,
    now: i64,
) -> Result<()> {
    let bytes: Vec<u8> = f.embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO feature_record (id, track_id, model_id, weights_checksum, preprocessing_version,
             source_fingerprint, segment_start_ms, segment_end_ms, dims, embedding, tempo, musical_key,
             loudness_lufs, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            new_id(),
            track_id,
            f.version.model_id,
            f.version.weights_checksum,
            f.version.preprocessing_version,
            format!("shared:{recording_id}"),
            f.embedding.len() as i64,
            bytes,
            f.tempo_bpm.map(|t| t as f64),
            f.key_camelot,
            f.loudness_lufs.map(|l| l as f64),
            now
        ],
    )?;
    Ok(())
}

impl Handler for ReuseHandler {
    fn kind(&self) -> &'static str {
        kinds::FEATURE_REUSE
    }

    fn run(&self, ctx: &mut JobCtx<'_>) -> std::result::Result<(), JobError> {
        let ReusePayload { track_id } = ctx.payload()?;
        let Some(central) = &self.central else {
            return Ok(());
        };
        if summary_embedding(ctx.conn, &track_id, &shared_version())?.is_some() {
            return Ok(());
        }
        let Some(key) = recording_key(ctx.conn, &track_id)? else {
            return Ok(());
        };
        let entry = match central.lookup(std::slice::from_ref(&key)) {
            Ok(mut r) => r.pop().flatten(),
            Err(e) => return Err(ctx.adapter_error(e)),
        };
        let Some(entry) = entry else { return Ok(()) };
        let v = shared_version();
        let version = FeatureVersion {
            model_id: v.model_id,
            weights_checksum: v.weights_checksum,
            preprocessing_version: v.preprocessing_version,
        };
        if !entry.feature_versions.contains(&version) {
            return Ok(());
        }
        let found = central
            .features(&entry.recording_id, &version)
            .map_err(|e| ctx.adapter_error(e))?;
        if let (Some(f), true) = (found.features, found.agreeing_contributors >= MIN_AGREEMENT) {
            if f.version == version && version.shared_dims() == Some(f.embedding.len()) {
                store_shared_features(ctx.conn, &track_id, &entry.recording_id, &f, ctx.now())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "sharing_tests.rs"]
mod tests;
