//! Read-only tag extraction. Nothing here ever writes to an audio file.

use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::domain::Field;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileTags {
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
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
}

impl FileTags {
    /// Values in the shape `meta::set_extracted` expects. Every field is
    /// listed so a tag removed from the file is removed here too.
    pub fn fields(&self) -> Vec<(Field, Option<String>)> {
        vec![
            (Field::Artist, self.artist.clone()),
            (Field::Title, self.title.clone()),
            (Field::Mix, self.mix.clone()),
            (Field::Label, self.label.clone()),
            (Field::Release, self.release.clone()),
            (Field::TrackNumber, self.track_number.clone()),
            (Field::Year, self.year.map(|y| y.to_string())),
            (Field::Genre, self.genre.clone()),
            (Field::Tempo, self.tempo.map(|t| format!("{t}"))),
            (Field::MusicalKey, self.musical_key.clone()),
        ]
    }
}

fn clean(s: Option<&str>) -> Option<String> {
    s.map(|v| v.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn first_string(tag: &Tag, keys: &[ItemKey]) -> Option<String> {
    keys.iter().find_map(|k| clean(tag.get_string(*k)))
}

/// Words that mark a parenthesised suffix as a mix or version name.
const MIX_WORDS: &[&str] = &[
    "mix",
    "remix",
    "edit",
    "dub",
    "version",
    "rework",
    "remaster",
    "remastered",
    "instrumental",
    "vip",
    "bootleg",
    "extended",
    "radio",
    "club",
    "acapella",
    "a cappella",
    "live",
    "reprise",
    "refix",
    "flip",
];

/// Split `"Title (Extended Mix)"` into `("Title", Some("Extended Mix"))`.
/// Only a trailing bracketed group containing a mix word is split off, so
/// titles such as "Song (Part 2)" are left alone.
pub fn split_title_mix(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    let (open, close) = match t.chars().last() {
        Some(')') => ('(', ')'),
        Some(']') => ('[', ']'),
        _ => return (t.to_string(), None),
    };
    let mut depth = 0;
    let mut start = None;
    for (i, c) in t.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                start = Some(i);
                break;
            }
        }
    }
    let Some(start) = start else {
        return (t.to_string(), None);
    };
    let inner = t[start + 1..t.len() - 1].trim();
    let base = t[..start].trim();
    let lower = inner.to_lowercase();
    let is_mix = MIX_WORDS
        .iter()
        .any(|w| lower == *w || lower.ends_with(&format!(" {w}")) || lower.starts_with(&format!("{w} ")));
    if is_mix && !base.is_empty() && !inner.is_empty() {
        (base.to_string(), Some(inner.to_string()))
    } else {
        (t.to_string(), None)
    }
}

fn parse_year(s: &str) -> Option<i64> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|y| (1000..=9999).contains(y))
}

fn parse_tempo(s: &str) -> Option<f64> {
    s.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|t| *t > 20.0 && *t < 400.0)
}

/// Read tags and stream properties. Returns `Err` with a readable message if
/// the container cannot be parsed at all.
pub fn read(path: &Path) -> Result<FileTags, String> {
    let file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let props = file.properties();
    let mut out = FileTags {
        duration_ms: Some(props.duration().as_millis() as i64).filter(|d| *d > 0),
        bitrate_kbps: props.audio_bitrate().map(i64::from),
        sample_rate: props.sample_rate().map(i64::from),
        channels: props.channels().map(i64::from),
        ..Default::default()
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Ok(out);
    };
    out.artist = clean(tag.artist().as_deref());
    if let Some(title) = clean(tag.title().as_deref()) {
        let (base, mix) = split_title_mix(&title);
        out.title = Some(base);
        out.mix = mix;
    }
    out.release = clean(tag.album().as_deref());
    out.genre = clean(tag.genre().as_deref());
    out.label = first_string(tag, &[ItemKey::Label, ItemKey::Publisher]);
    out.track_number = tag.track().map(|n| n.to_string());
    out.year = first_string(
        tag,
        &[ItemKey::RecordingDate, ItemKey::Year, ItemKey::ReleaseDate],
    )
    .and_then(|y| parse_year(&y));
    out.tempo = first_string(tag, &[ItemKey::Bpm, ItemKey::IntegerBpm]).and_then(|t| parse_tempo(&t));
    out.musical_key = first_string(tag, &[ItemKey::InitialKey]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_common_mix_suffixes() {
        assert_eq!(
            split_title_mix("Night Signal (Extended Mix)"),
            ("Night Signal".into(), Some("Extended Mix".into()))
        );
        assert_eq!(
            split_title_mix("Strings of Life [Kerri Chandler Remix]"),
            ("Strings of Life".into(), Some("Kerri Chandler Remix".into()))
        );
        assert_eq!(
            split_title_mix("Can You Feel It (Dub)"),
            ("Can You Feel It".into(), Some("Dub".into()))
        );
        assert_eq!(
            split_title_mix("A (B (C) Edit)"),
            ("A".into(), Some("B (C) Edit".into()))
        );
    }

    #[test]
    fn leaves_non_mix_brackets_alone() {
        assert_eq!(split_title_mix("Song (Part 2)"), ("Song (Part 2)".into(), None));
        assert_eq!(split_title_mix("Plain Title"), ("Plain Title".into(), None));
        assert_eq!(split_title_mix("(Mix)"), ("(Mix)".into(), None));
        assert_eq!(
            split_title_mix("Unbalanced (Mix"),
            ("Unbalanced (Mix".into(), None)
        );
    }

    #[test]
    fn parses_years_and_tempos() {
        assert_eq!(parse_year("2024-03-01"), Some(2024));
        assert_eq!(parse_year("n/a"), None);
        assert_eq!(parse_tempo("124"), Some(124.0));
        assert_eq!(parse_tempo("123,5"), Some(123.5));
        assert_eq!(parse_tempo("0"), None);
    }
}
