//! Library listing with full-text search and filters.

use rusqlite::{params_from_iter, types::Value, Connection};
use serde::{Deserialize, Serialize};

use crate::domain::Availability;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RatingFilter {
    Unrated,
    ThumbsDown,
    /// At least this many stars.
    MinStars(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    #[default]
    Artist,
    Title,
    Tempo,
    Key,
    Added,
    Rating,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryQuery {
    /// Free text matched against artist, title, mix, label and release.
    pub text: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub label: Option<String>,
    pub rating: Option<RatingFilter>,
    pub tempo_min: Option<f64>,
    pub tempo_max: Option<f64>,
    pub key: Option<String>,
    pub availability: Option<Availability>,
    pub playlist_id: Option<String>,
    pub sort: SortBy,
    pub descending: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryRow {
    pub track_id: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub label: Option<String>,
    pub release: Option<String>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub tempo: Option<f64>,
    pub musical_key: Option<String>,
    pub file_id: Option<String>,
    pub path: Option<String>,
    pub availability: Option<Availability>,
    pub availability_reason: Option<String>,
    pub duration_ms: Option<i64>,
    pub format: Option<String>,
    pub file_count: i64,
    /// `thumbs_down`, `star1`, `star2` or `star3`.
    pub rating: Option<String>,
    pub playlist_count: i64,
    pub analysed: bool,
    pub kept: bool,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryPage {
    pub rows: Vec<LibraryRow>,
    pub total: i64,
}

/// Turn user input into a safe FTS5 query: every word must match as a
/// prefix. Quotes and operators in the input are treated as text.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn like(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Tracks that are part of the library: any with an imported or archived
/// file. Candidates in staging appear in Review, not here.
const LIBRARY_MEMBER: &str =
    "EXISTS (SELECT 1 FROM audio_file lf WHERE lf.track_id = m.track_id AND lf.origin IN ('imported', 'archived'))";

pub fn search(conn: &Connection, q: &LibraryQuery) -> Result<LibraryPage> {
    let mut wheres = vec![LIBRARY_MEMBER.to_string()];
    let mut args: Vec<Value> = Vec::new();

    if let Some(fts) = q.text.as_deref().and_then(fts_query) {
        wheres.push("m.rowid IN (SELECT rowid FROM track_fts WHERE track_fts MATCH ?)".into());
        args.push(fts.into());
    }
    for (col, val) in [
        ("m.artist", &q.artist),
        ("m.title", &q.title),
        ("m.mix", &q.mix),
        ("m.label", &q.label),
    ] {
        if let Some(v) = val.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            wheres.push(format!("{col} LIKE ? ESCAPE '\\'"));
            args.push(like(v).into());
        }
    }
    if let Some(t) = q.tempo_min {
        wheres.push("m.tempo >= ?".into());
        args.push(t.into());
    }
    if let Some(t) = q.tempo_max {
        wheres.push("m.tempo <= ?".into());
        args.push(t.into());
    }
    if let Some(k) = q.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        wheres.push("m.musical_key = ? COLLATE NOCASE".into());
        args.push(k.to_string().into());
    }
    match q.rating {
        Some(RatingFilter::Unrated) => wheres.push("er.kind IS NULL".into()),
        Some(RatingFilter::ThumbsDown) => wheres.push("er.kind = 'thumbs_down'".into()),
        Some(RatingFilter::MinStars(n)) => {
            let kinds: Vec<String> = (n.clamp(1, 3)..=3).map(|s| format!("'star{s}'")).collect();
            wheres.push(format!("er.kind IN ({})", kinds.join(", ")));
        }
        None => {}
    }
    if let Some(a) = q.availability {
        wheres.push("f.availability = ?".into());
        args.push(a.as_str().to_string().into());
    }
    if let Some(p) = &q.playlist_id {
        wheres.push(
            "EXISTS (SELECT 1 FROM playlist_entry pe WHERE pe.track_id = m.track_id AND pe.playlist_id = ?)"
                .into(),
        );
        args.push(p.clone().into());
    }

    let order_col = match q.sort {
        SortBy::Artist => "m.artist COLLATE NOCASE",
        SortBy::Title => "m.title COLLATE NOCASE",
        SortBy::Tempo => "m.tempo",
        SortBy::Key => "m.musical_key",
        SortBy::Added => "t.created_at",
        SortBy::Rating => {
            "CASE er.kind WHEN 'star3' THEN 3 WHEN 'star2' THEN 2 WHEN 'star1' THEN 1 WHEN 'thumbs_down' THEN -1 ELSE 0 END"
        }
    };
    let dir = if q.descending { "DESC" } else { "ASC" };
    let file_join = "LEFT JOIN audio_file f ON f.id = (
             SELECT id FROM audio_file x WHERE x.track_id = m.track_id
             ORDER BY x.is_primary DESC, (x.availability = 'available') DESC, x.created_at LIMIT 1)";
    let rating_join = "LEFT JOIN effective_rating er ON er.track_id = m.track_id";
    let from = format!("FROM track_meta m JOIN track t ON t.id = m.track_id {file_join} {rating_join}");
    let where_sql = wheres.join(" AND ");

    // The count only pays for the joins its filters need.
    let count_from = format!(
        "FROM track_meta m {} {}",
        if q.availability.is_some() { file_join } else { "" },
        if q.rating.is_some() { rating_join } else { "" }
    );
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) {count_from} WHERE {where_sql}"),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?;

    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let offset = q.offset.unwrap_or(0).max(0);
    let sql = format!(
        "SELECT m.track_id, m.artist, m.title, m.mix, m.label, m.release, m.year, m.genre, m.tempo,
                m.musical_key, f.id, f.path, f.availability, f.availability_reason, f.duration_ms, f.format,
                (SELECT COUNT(*) FROM audio_file c WHERE c.track_id = m.track_id),
                er.kind,
                (SELECT COUNT(DISTINCT playlist_id) FROM playlist_entry pe WHERE pe.track_id = m.track_id),
                EXISTS (SELECT 1 FROM feature_record fr WHERE fr.track_id = m.track_id),
                EXISTS (SELECT 1 FROM keep_decision k WHERE k.track_id = m.track_id),
                t.created_at
         {from} WHERE {where_sql}
         ORDER BY {order_col} IS NULL, {order_col} {dir}, m.artist COLLATE NOCASE, m.title COLLATE NOCASE
         LIMIT {limit} OFFSET {offset}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            Ok(LibraryRow {
                track_id: r.get(0)?,
                artist: r.get(1)?,
                title: r.get(2)?,
                mix: r.get(3)?,
                label: r.get(4)?,
                release: r.get(5)?,
                year: r.get(6)?,
                genre: r.get(7)?,
                tempo: r.get(8)?,
                musical_key: r.get(9)?,
                file_id: r.get(10)?,
                path: r.get(11)?,
                availability: r.get(12)?,
                availability_reason: r.get(13)?,
                duration_ms: r.get(14)?,
                format: r.get(15)?,
                file_count: r.get(16)?,
                rating: r.get(17)?,
                playlist_count: r.get(18)?,
                analysed: r.get(19)?,
                kept: r.get(20)?,
                added_at: r.get(21)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(LibraryPage { rows, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_escapes_user_input() {
        assert_eq!(fts_query("kerri chan").as_deref(), Some("\"kerri\"* \"chan\"*"));
        assert_eq!(fts_query("\"OR\" NEAR(").as_deref(), Some("\"OR\"* \"NEAR\"*"));
        assert_eq!(fts_query("  -- ").as_deref(), None);
    }

    #[test]
    fn like_escapes_wildcards() {
        assert_eq!(like("50%_off"), "%50\\%\\_off%");
    }
}
