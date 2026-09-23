//! Playlist export for DJ software: M3U8 and Rekordbox XML.
//!
//! Every entry is checked first; missing, unreadable and temporary files
//! are reported before anything is written. Only what the library knows is
//! exported: no cue points or beat grids, and tempo and key only when they
//! come from file tags or the user's corrections, never from this app's
//! own estimates.

use serde::Serialize;

use crate::domain::{Availability, FileOrigin};
use crate::meta::TrackMeta;
use crate::{library, meta, playlists, Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportTrack {
    pub position: i64,
    pub track_id: String,
    pub path: String,
    pub meta: TrackMeta,
    pub duration_ms: Option<i64>,
    pub size_bytes: i64,
    pub format: Option<String>,
    /// From tags or the user's corrections only.
    pub tempo: Option<f64>,
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Problem {
    /// No copy of the track's audio is available.
    Missing { position: i64, track: String },
    /// Still in temporary storage; archiving would move it and break the exported path.
    Temporary { position: i64, track: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Prepared {
    pub name: String,
    pub tracks: Vec<ExportTrack>,
    pub problems: Vec<Problem>,
}

fn label(m: &TrackMeta) -> String {
    let mut s = match (&m.artist, &m.title) {
        (Some(a), Some(t)) => format!("{a} - {t}"),
        (None, Some(t)) => t.clone(),
        (Some(a), None) => a.clone(),
        (None, None) => "Untitled".into(),
    };
    if let Some(mix) = &m.mix {
        s.push_str(&format!(" ({mix})"));
    }
    s
}

/// Tempo and key are exported only when they did not come from analysis.
fn trusted(conn: &rusqlite::Connection, track_id: &str) -> Result<(Option<f64>, Option<String>)> {
    let (mut tempo, mut key) = (None, None);
    for p in meta::provenance(conn, track_id)? {
        if p.source == "analysis" && !p.corrected {
            continue;
        }
        match p.field {
            crate::domain::Field::Tempo => tempo = p.value.and_then(|v| v.parse().ok()),
            crate::domain::Field::MusicalKey => key = p.value.filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    Ok((tempo, key))
}

pub fn prepare(conn: &rusqlite::Connection, playlist_id: &str) -> Result<Prepared> {
    let name = playlists::list(conn)?
        .into_iter()
        .find(|p| p.id == playlist_id)
        .ok_or_else(|| Error::NotFound(format!("playlist {playlist_id}")))?
        .name;
    let mut tracks = vec![];
    let mut problems = vec![];
    for e in playlists::entries(conn, playlist_id)? {
        let m = meta::effective(conn, &e.track_id)?;
        let file = library::playable_file(conn, &e.track_id)?;
        let Some(file) = file.filter(|f| f.availability == Availability::Available) else {
            problems.push(Problem::Missing {
                position: e.position,
                track: label(&m),
            });
            continue;
        };
        if file.origin == FileOrigin::Staged {
            problems.push(Problem::Temporary {
                position: e.position,
                track: label(&m),
            });
        }
        let (tempo, key) = trusted(conn, &e.track_id)?;
        tracks.push(ExportTrack {
            position: e.position,
            track_id: e.track_id,
            path: file.path,
            meta: m,
            duration_ms: file.duration_ms,
            size_bytes: file.size_bytes,
            format: file.format,
            tempo,
            key,
        });
    }
    Ok(Prepared {
        name,
        tracks,
        problems,
    })
}

/// One line of text, safe inside M3U comments and XML attributes.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

pub fn m3u8(tracks: &[ExportTrack]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let secs = t.duration_ms.map(|d| (d + 500) / 1000).unwrap_or(-1);
        out.push_str(&format!(
            "#EXTINF:{secs},{}\n{}\n",
            one_line(&label(&t.meta)),
            one_line(&t.path)
        ));
    }
    out
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in one_line(s).chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// A `file://localhost/` URI as Rekordbox writes them: forward slashes,
/// UTF-8 percent-encoded, drive letters kept (`file://localhost/C:/...`).
pub fn file_uri(path: &str) -> String {
    let normal = path.replace('\\', "/");
    let trimmed = normal.trim_start_matches('/');
    let mut out = String::from("file://localhost/");
    for (i, b) in trimmed.bytes().enumerate() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            // The colon after a Windows drive letter.
            || (b == b':' && i == 1);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn kind(t: &ExportTrack) -> &'static str {
    let ext = t.path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp3" => "MP3 File",
        "flac" => "FLAC File",
        "wav" => "WAV File",
        "aif" | "aiff" => "AIFF File",
        "m4a" | "mp4" => "M4A File",
        "ogg" => "OGG File",
        _ => "Audio File",
    }
}

pub fn rekordbox_xml(name: &str, tracks: &[ExportTrack]) -> String {
    // Each track appears once in the collection, however often the playlist repeats it.
    let mut ids: Vec<&str> = vec![];
    let mut collection = String::new();
    for t in tracks {
        if ids.contains(&t.track_id.as_str()) {
            continue;
        }
        ids.push(&t.track_id);
        let attr = |k: &str, v: Option<String>| match v {
            Some(v) if !v.is_empty() => format!(" {k}=\"{}\"", xml_escape(&v)),
            _ => String::new(),
        };
        collection.push_str(&format!(
            "    <TRACK TrackID=\"{}\"{}{}{}{}{}{}{}{}{}{}{} Kind=\"{}\" Size=\"{}\" Location=\"{}\"/>\n",
            ids.len(),
            attr("Name", t.meta.title.clone()),
            attr("Artist", t.meta.artist.clone()),
            attr("Mix", t.meta.mix.clone()),
            attr("Album", t.meta.release.clone()),
            attr("Label", t.meta.label.clone()),
            attr("Genre", t.meta.genre.clone()),
            attr("Year", t.meta.year.map(|y| y.to_string())),
            attr("TrackNumber", t.meta.track_number.clone()),
            attr("TotalTime", t.duration_ms.map(|d| ((d + 500) / 1000).to_string())),
            attr("AverageBpm", t.tempo.map(|b| format!("{b:.2}"))),
            attr("Tonality", t.key.clone()),
            kind(t),
            t.size_bytes.max(0),
            xml_escape(&file_uri(&t.path)),
        ));
    }
    let mut playlist = String::new();
    for t in tracks {
        let key = ids.iter().position(|id| *id == t.track_id).unwrap() + 1;
        playlist.push_str(&format!("        <TRACK Key=\"{key}\"/>\n"));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <DJ_PLAYLISTS Version=\"1.0.0\">\n  \
         <PRODUCT Name=\"Crate Digger\" Version=\"{}\" Company=\"Crate Digger\"/>\n  \
         <COLLECTION Entries=\"{}\">\n{collection}  </COLLECTION>\n  \
         <PLAYLISTS>\n    <NODE Type=\"0\" Name=\"ROOT\" Count=\"1\">\n      \
         <NODE Name=\"{}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">\n{playlist}      </NODE>\n    \
         </NODE>\n  </PLAYLISTS>\n</DJ_PLAYLISTS>\n",
        env!("CARGO_PKG_VERSION"),
        ids.len(),
        xml_escape(name),
        tracks.len(),
    )
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
