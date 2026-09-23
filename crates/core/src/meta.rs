//! Track records and metadata precedence.
//!
//! Extracted values are stored per source in `field_value`. User corrections
//! are stored separately in `field_correction` and always win. The effective
//! result is materialised into `track_meta` (which feeds full-text search)
//! every time either side changes.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::domain::Field;
use crate::util::{new_id, now_ms};
use crate::Result;

/// Lower number wins when several automatic sources disagree.
fn source_priority(source: &str) -> i64 {
    match source {
        "tags" => 0,
        "discogs" => 10,
        "analysis" => 20,
        // Tags inside downloaded files come from strangers; identified
        // source metadata is preferred.
        "download_tags" => 200,
        _ => 100,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub label: Option<String>,
    pub release: Option<String>,
    pub track_number: Option<String>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub tempo: Option<f64>,
    pub musical_key: Option<String>,
}

/// Where an effective field value came from, for display next to edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldProvenance {
    pub field: Field,
    pub value: Option<String>,
    pub source: String,
    pub corrected: bool,
}

pub fn create_track(conn: &Connection) -> Result<String> {
    let id = new_id();
    let now = now_ms();
    conn.execute(
        "INSERT INTO track (id, created_at, updated_at) VALUES (?1, ?2, ?2)",
        params![id, now],
    )?;
    conn.execute("INSERT INTO track_meta (track_id) VALUES (?1)", params![id])?;
    Ok(id)
}

/// Record automatically extracted values from `source`. A `None` value
/// removes that source's earlier value for the field.
pub fn set_extracted(
    conn: &Connection,
    track_id: &str,
    source: &str,
    values: &[(Field, Option<String>)],
) -> Result<()> {
    let now = now_ms();
    for (field, value) in values {
        match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => {
                conn.execute(
                    "INSERT INTO field_value (track_id, field, source, value, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (track_id, field, source)
                     DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![track_id, field, source, v, now],
                )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM field_value WHERE track_id = ?1 AND field = ?2 AND source = ?3",
                    params![track_id, field, source],
                )?;
            }
        }
    }
    refresh_effective(conn, track_id)
}

/// Store a user correction. `Some("")` records that the user cleared the
/// field; `None` removes the correction so automatic values apply again.
pub fn set_correction(conn: &Connection, track_id: &str, field: Field, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => {
            conn.execute(
                "INSERT INTO field_correction (track_id, field, value, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (track_id, field)
                 DO UPDATE SET value = excluded.value, created_at = excluded.created_at",
                params![track_id, field, v.trim(), now_ms()],
            )?;
        }
        None => {
            conn.execute(
                "DELETE FROM field_correction WHERE track_id = ?1 AND field = ?2",
                params![track_id, field],
            )?;
        }
    }
    refresh_effective(conn, track_id)
}

/// Resolve one field: correction first, then the highest-priority source,
/// then the most recent value.
fn resolve(conn: &Connection, track_id: &str, field: Field) -> Result<Option<FieldProvenance>> {
    let correction: Option<String> = conn
        .query_row(
            "SELECT value FROM field_correction WHERE track_id = ?1 AND field = ?2",
            params![track_id, field],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(v) = correction {
        return Ok(Some(FieldProvenance {
            field,
            value: if v.is_empty() { None } else { Some(v) },
            source: "user".into(),
            corrected: true,
        }));
    }
    let mut stmt = conn.prepare_cached(
        "SELECT source, value, updated_at FROM field_value WHERE track_id = ?1 AND field = ?2",
    )?;
    let rows = stmt
        .query_map(params![track_id, field], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .min_by_key(|(source, _, updated)| (source_priority(source), -updated))
        .map(|(source, value, _)| FieldProvenance {
            field,
            value: Some(value),
            source,
            corrected: false,
        }))
}

pub fn provenance(conn: &Connection, track_id: &str) -> Result<Vec<FieldProvenance>> {
    let mut out = Vec::new();
    for field in Field::ALL {
        if let Some(p) = resolve(conn, track_id, *field)? {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn refresh_effective(conn: &Connection, track_id: &str) -> Result<()> {
    let mut m = TrackMeta::default();
    for field in Field::ALL {
        let value = resolve(conn, track_id, *field)?.and_then(|p| p.value);
        match field {
            Field::Artist => m.artist = value,
            Field::Title => m.title = value,
            Field::Mix => m.mix = value,
            Field::Label => m.label = value,
            Field::Release => m.release = value,
            Field::TrackNumber => m.track_number = value,
            Field::Year => m.year = value.and_then(|v| v.parse().ok()),
            Field::Genre => m.genre = value,
            Field::Tempo => m.tempo = value.and_then(|v| v.parse().ok()),
            Field::MusicalKey => m.musical_key = value,
        }
    }
    conn.execute(
        "INSERT INTO track_meta (track_id, artist, title, mix, label, release, track_number,
                                 year, genre, tempo, musical_key, title_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (track_id) DO UPDATE SET
             artist = excluded.artist, title = excluded.title, mix = excluded.mix,
             label = excluded.label, release = excluded.release,
             track_number = excluded.track_number, year = excluded.year,
             genre = excluded.genre, tempo = excluded.tempo,
             musical_key = excluded.musical_key, title_key = excluded.title_key",
        params![
            track_id,
            m.artist,
            m.title,
            m.mix,
            m.label,
            m.release,
            m.track_number,
            m.year,
            m.genre,
            m.tempo,
            m.musical_key,
            m.title.as_deref().map(crate::identity::normalize::title_key)
        ],
    )?;
    conn.execute(
        "UPDATE track SET updated_at = ?2 WHERE id = ?1",
        params![track_id, now_ms()],
    )?;
    Ok(())
}

pub fn effective(conn: &Connection, track_id: &str) -> Result<TrackMeta> {
    conn.query_row(
        "SELECT artist, title, mix, label, release, track_number, year, genre, tempo, musical_key
         FROM track_meta WHERE track_id = ?1",
        params![track_id],
        |r| {
            Ok(TrackMeta {
                artist: r.get(0)?,
                title: r.get(1)?,
                mix: r.get(2)?,
                label: r.get(3)?,
                release: r.get(4)?,
                track_number: r.get(5)?,
                year: r.get(6)?,
                genre: r.get(7)?,
                tempo: r.get(8)?,
                musical_key: r.get(9)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| crate::Error::NotFound(format!("track {track_id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn user_correction_survives_later_automatic_update() {
        let conn = open_in_memory().unwrap();
        let t = create_track(&conn).unwrap();
        set_extracted(
            &conn,
            &t,
            "tags",
            &[
                (Field::Title, s("Strings Of Lfe")),
                (Field::Artist, s("Kerri Chandler")),
            ],
        )
        .unwrap();
        set_correction(&conn, &t, Field::Title, Some("Strings Of Life")).unwrap();

        // A later re-import brings a different automatic value.
        set_extracted(
            &conn,
            &t,
            "tags",
            &[(Field::Title, s("Strings of Life (tagged)"))],
        )
        .unwrap();
        set_extracted(&conn, &t, "discogs", &[(Field::Title, s("Strings Of Life"))]).unwrap();

        let m = effective(&conn, &t).unwrap();
        assert_eq!(m.title.as_deref(), Some("Strings Of Life"));
        assert_eq!(m.artist.as_deref(), Some("Kerri Chandler"));

        // The automatic value is still stored for provenance.
        let tagged: String = conn
            .query_row(
                "SELECT value FROM field_value WHERE track_id = ?1 AND field = 'title' AND source = 'tags'",
                [&t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tagged, "Strings of Life (tagged)");
    }

    #[test]
    fn clearing_a_field_is_a_correction_and_removing_it_reverts() {
        let conn = open_in_memory().unwrap();
        let t = create_track(&conn).unwrap();
        set_extracted(&conn, &t, "tags", &[(Field::Mix, s("Original Mix"))]).unwrap();
        set_correction(&conn, &t, Field::Mix, Some("")).unwrap();
        assert_eq!(effective(&conn, &t).unwrap().mix, None);
        set_correction(&conn, &t, Field::Mix, None).unwrap();
        assert_eq!(effective(&conn, &t).unwrap().mix.as_deref(), Some("Original Mix"));
    }

    #[test]
    fn higher_priority_source_wins() {
        let conn = open_in_memory().unwrap();
        let t = create_track(&conn).unwrap();
        set_extracted(&conn, &t, "fake_source", &[(Field::Label, s("Unknown"))]).unwrap();
        set_extracted(&conn, &t, "tags", &[(Field::Label, s("Transmat"))]).unwrap();
        assert_eq!(effective(&conn, &t).unwrap().label.as_deref(), Some("Transmat"));
    }

    #[test]
    fn effective_metadata_is_searchable() {
        let conn = open_in_memory().unwrap();
        let t = create_track(&conn).unwrap();
        set_extracted(&conn, &t, "tags", &[(Field::Artist, s("Róisín Murphy"))]).unwrap();
        let found: String = conn
            .query_row(
                "SELECT m.track_id FROM track_fts f JOIN track_meta m ON m.rowid = f.rowid
                 WHERE track_fts MATCH 'roisin'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, t);
    }
}
