//! Domain enums shared by storage, jobs and the UI.
//!
//! Each enum maps to the exact strings used in SQL CHECK constraints.

use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                let s = v.as_str()?;
                $name::parse(s).ok_or_else(|| {
                    rusqlite::types::FromSqlError::Other(
                        format!("unknown {} value {s:?}", stringify!($name)).into(),
                    )
                })
            }
        }
    };
}

string_enum!(
    /// Metadata fields that can be extracted and corrected.
    Field {
        Artist => "artist",
        Title => "title",
        Mix => "mix",
        Label => "label",
        Release => "release",
        TrackNumber => "track_number",
        Year => "year",
        Genre => "genre",
        Tempo => "tempo",
        MusicalKey => "musical_key",
    }
);

string_enum!(
    /// Position of a candidate in the discovery pipeline.
    Stage {
        Candidate => "candidate",
        Identified => "identified",
        AcquisitionQueued => "acquisition_queued",
        Downloading => "downloading",
        Validating => "validating",
        Analysing => "analysing",
        Ready => "ready",
        Reviewed => "reviewed",
    }
);

string_enum!(
    /// Whether a candidate is progressing. Anything other than `Active`
    /// carries a human-readable reason.
    CandidateStatus {
        Active => "active",
        Paused => "paused",
        Blocked => "blocked",
        Failed => "failed",
        Cancelled => "cancelled",
    }
);

string_enum!(
    JobState {
        Queued => "queued",
        Running => "running",
        Paused => "paused",
        Blocked => "blocked",
        Failed => "failed",
        Cancelled => "cancelled",
        Done => "done",
    }
);

string_enum!(
    /// Entries in the append-only preference log.
    RatingKind {
        ThumbsDown => "thumbs_down",
        Star1 => "star1",
        Star2 => "star2",
        Star3 => "star3",
        Skip => "skip",
        Undo => "undo",
    }
);

string_enum!(
    Availability {
        Available => "available",
        Missing => "missing",
        Corrupt => "corrupt",
    }
);

string_enum!(
    FileOrigin {
        Imported => "imported",
        Staged => "staged",
        Archived => "archived",
    }
);

string_enum!(
    SeedKind {
        Artist => "artist",
        Label => "label",
        Dj => "dj",
        Track => "track",
    }
);

impl JobState {
    /// States that need a reason attached.
    pub fn needs_reason(self) -> bool {
        !matches!(self, JobState::Queued | JobState::Running | JobState::Done)
    }
}

/// Effective preference for a track after replaying its rating events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    ThumbsDown,
    Stars(u8),
}

impl Preference {
    pub fn from_kind(kind: RatingKind) -> Option<Self> {
        match kind {
            RatingKind::ThumbsDown => Some(Preference::ThumbsDown),
            RatingKind::Star1 => Some(Preference::Stars(1)),
            RatingKind::Star2 => Some(Preference::Stars(2)),
            RatingKind::Star3 => Some(Preference::Stars(3)),
            RatingKind::Skip | RatingKind::Undo => None,
        }
    }

    /// Signed strength used by ranking: -1 for dislike, 1..=3 for stars.
    pub fn weight(self) -> i32 {
        match self {
            Preference::ThumbsDown => -1,
            Preference::Stars(n) => n as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_strings_round_trip() {
        for s in Stage::ALL {
            assert_eq!(Stage::parse(s.as_str()), Some(*s));
        }
        for s in JobState::ALL {
            assert_eq!(JobState::parse(s.as_str()), Some(*s));
        }
        assert_eq!(Field::parse("nope"), None);
    }
}
