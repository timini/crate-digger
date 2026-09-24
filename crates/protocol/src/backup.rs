//! Private backups: one user's ratings, seeds, playlists and library
//! metadata. Never shared, never mixed with contributions, and only ever
//! visible to the account that uploaded them. Audio files, local paths and
//! credentials are not part of a snapshot.

use serde::{Deserialize, Serialize};

use crate::{Metadata, Problem, Validate};

pub const SNAPSHOT_VERSION: u32 = 1;
/// Largest snapshot the service stores.
pub const MAX_SNAPSHOT_BYTES: usize = 20 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub created_at_ms: i64,
    pub tracks: Vec<Track>,
    pub ratings: Vec<Rating>,
    pub seeds: Vec<Seed>,
    pub playlists: Vec<Playlist>,
}

/// A track as the user's library knows it. `id` is local to the snapshot;
/// restore matches it to files by metadata and fingerprint, then asks the
/// user to relink anything it cannot find.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: String,
    pub metadata: Metadata,
    pub fingerprint_hash: Option<String>,
    pub kept: bool,
    /// The track's audio file as it was, so restore can find it again.
    #[serde(default)]
    pub file: Option<FileRef>,
}

/// Enough to recognise the user's own file after a move: its name, size,
/// content hash and length. Never shared; only in the owner's backups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub name: String,
    pub size_bytes: u64,
    pub content_hash: String,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    pub track: String,
    /// `thumbs_down`, `star1`, `star2` or `star3`.
    pub kind: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seed {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub created_at_ms: i64,
    pub size_bytes: u64,
    pub version: u32,
}

impl Validate for Snapshot {
    fn validate(&self) -> Result<(), Problem> {
        let bad = |code: &str, m: &str| {
            Err(Problem {
                code: code.into(),
                message: m.into(),
            })
        };
        if self.version != SNAPSHOT_VERSION {
            return bad("snapshot_version", "unsupported snapshot version");
        }
        let ids: std::collections::HashSet<&str> = self.tracks.iter().map(|t| t.id.as_str()).collect();
        if ids.len() != self.tracks.len() {
            return bad("duplicate_track", "track ids must be unique");
        }
        if self.ratings.iter().any(|r| !ids.contains(r.track.as_str()))
            || self
                .playlists
                .iter()
                .flat_map(|p| &p.tracks)
                .any(|t| !ids.contains(t.as_str()))
        {
            return bad(
                "unknown_track",
                "ratings and playlists may only refer to tracks in the snapshot",
            );
        }
        if self.tracks.iter().filter_map(|t| t.file.as_ref()).any(|f| {
            f.name.is_empty()
                || f.name.contains(['/', '\\'])
                || f.name.len() > 255
                || f.content_hash.len() > 128
        }) {
            return bad(
                "file_ref",
                "file references are a bare file name, a size and a hash",
            );
        }
        if self
            .ratings
            .iter()
            .any(|r| !matches!(r.kind.as_str(), "thumbs_down" | "star1" | "star2" | "star3"))
        {
            return bad("rating_kind", "unknown rating");
        }
        Ok(())
    }
}
