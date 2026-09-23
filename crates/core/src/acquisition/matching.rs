//! Choosing a download from search results.
//!
//! A result is acceptable when its file name carries the wanted title and
//! artist, names a compatible mix, is lossless or a 320 kbps MP3, and is not
//! a preview, a pitched copy or a cut from a DJ mix. A download starts on its
//! own only when acceptable copies exist and agree on length; everything
//! else goes to the user with the options ranked. Calibrated in
//! `tests/acquisition_calibration.rs`.

use serde::{Deserialize, Serialize};

use crate::adapters::{AcquisitionQuery, SearchResult};
use crate::identity::normalize::{classify_mix, fold, parse_artists, title_key, MixClass};

/// Acceptable copies whose lengths differ by more than this may be
/// different versions labelled the same way.
pub const LENGTH_SPREAD_MS: u64 = 10_000;
/// Anything shorter is a preview or a snippet.
pub const MIN_LENGTH_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Lossless,
    Mp3_320,
    /// Needs the user's choice.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixFit {
    Compatible,
    /// One side names a version the other does not; could be either.
    Uncertain,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assessed {
    pub result: SearchResult,
    pub quality: Quality,
    pub mix: MixFit,
    pub acceptable: bool,
    /// Why it is not acceptable, in words for the choice screen.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Download `ranked[0]` without asking.
    Auto,
    /// Ask the user. `recommended` indexes `ranked`.
    Choose {
        recommended: Option<usize>,
        why: String,
    },
    Nothing {
        why: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// Results that name the wanted track, best first.
    pub ranked: Vec<Assessed>,
    pub decision: Decision,
}

const LOSSLESS: &[&str] = &["flac", "wav", "aiff", "aif", "alac", "ape", "wv"];
/// Words that mark a copy DJs should not get by accident.
const BAD_COPY: &[(&str, &str)] = &[
    ("preview", "a preview"),
    ("snippet", "a snippet"),
    ("sample", "a sample"),
    ("pitched", "pitched"),
    ("sped", "sped up"),
    ("slowed", "slowed down"),
    ("nightcore", "sped up"),
    ("mixed", "cut from a DJ mix"),
    ("live", "a live recording"),
    ("karaoke", "karaoke"),
];

fn basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if name.len() - i <= 5 => &name[..i],
        _ => name,
    }
}

fn extension(result: &SearchResult) -> String {
    result
        .format
        .clone()
        .unwrap_or_else(|| {
            let name = basename(&result.filename);
            name.rfind('.')
                .map(|i| name[i + 1..].to_string())
                .unwrap_or_default()
        })
        .to_ascii_lowercase()
}

pub fn quality(result: &SearchResult) -> Quality {
    let ext = extension(result);
    if LOSSLESS.contains(&ext.as_str()) {
        Quality::Lossless
    } else if ext == "mp3" && result.bitrate_kbps.is_some_and(|b| b >= 320) {
        Quality::Mp3_320
    } else {
        Quality::Other
    }
}

/// Splits trailing bracket groups off a file name: "Title (Dub) [CAT001]"
/// gives ("Title", ["Dub", "CAT001"]), last group first.
fn trailing_groups(stem: &str) -> (String, Vec<String>) {
    let mut base = stem.trim().to_string();
    let mut groups = vec![];
    for _ in 0..3 {
        let close = match base.chars().last() {
            Some(c @ (')' | ']')) => c,
            _ => break,
        };
        let open = if close == ')' { '(' } else { '[' };
        let Some(start) = base.rfind(open) else { break };
        if start == 0 {
            break;
        }
        groups.push(base[start + 1..base.len() - 1].trim().to_string());
        base.truncate(start);
        base = base.trim_end().to_string();
    }
    (base, groups)
}

/// A bracket that describes the copy rather than naming the track.
fn descriptive(group: &str) -> bool {
    let words = fold(group);
    let catalogue = !group.contains(' ') && group.chars().any(|c| c.is_ascii_digit());
    !matches!(classify_mix(Some(group)), MixClass::Other(_))
        || BAD_COPY.iter().any(|(w, _)| contains_words(&words, w))
        || catalogue
        || words.ends_with(" records")
        || words.ends_with(" recordings")
}

/// The mix named in a file name's trailing brackets, if any.
fn file_mix(stem: &str) -> Option<String> {
    trailing_groups(stem)
        .1
        .into_iter()
        .find(|g| !matches!(classify_mix(Some(g)), MixClass::Other(_)))
}

/// The title part of a file name: without descriptive brackets, the artist
/// before " - ", and a leading track number or vinyl position.
fn file_title(stem: &str) -> String {
    let (base, groups) = trailing_groups(stem);
    let mut t = base;
    // Brackets that belong to the title, such as "(Part 2)", stay.
    for g in groups.iter().rev().filter(|g| !descriptive(g)) {
        t.push_str(&format!(" ({g})"));
    }
    let last = t.rsplit(" - ").next().unwrap_or(&t).trim();
    let mut words = last.split_whitespace().peekable();
    // "03 Title", "03. Title", "A1 Title"
    if let Some(first) = words.peek() {
        let bare = first.trim_end_matches(['.', ')']);
        let position = bare.len() <= 3
            && bare.chars().last().is_some_and(|c| c.is_ascii_digit())
            && bare.chars().skip(1).all(|c| c.is_ascii_digit());
        if position && last.split_whitespace().count() > 1 {
            words.next();
        }
    }
    words.collect::<Vec<_>>().join(" ")
}

pub fn mix_fit(wanted: &MixClass, found: &MixClass) -> MixFit {
    use MixClass::*;
    let original = |m: &MixClass| matches!(m, Unspecified | Original | Remaster);
    match (wanted, found) {
        // A remix or edit whose maker is named on one side only.
        (Remix(a), Remix(b)) | (Edit(a), Edit(b)) if a.is_none() != b.is_none() => return MixFit::Uncertain,
        _ if wanted.same_recording_as(found) => return MixFit::Compatible,
        _ => {}
    }
    match (original(wanted), original(found)) {
        // The file does not say which version it is.
        (false, true) if matches!(found, Unspecified) => MixFit::Uncertain,
        // Longer or shorter cuts of the original arrangement.
        (true, false) if matches!(found, Extended | RadioEdit | Club) => MixFit::Uncertain,
        _ => MixFit::Incompatible,
    }
}

fn contains_words(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && format!(" {haystack} ").contains(&format!(" {needle} "))
}

pub fn assess_one(query: &AcquisitionQuery, result: &SearchResult) -> Option<Assessed> {
    let name = stem(basename(&result.filename));
    let folded_name = fold(name);
    let folded_path = fold(&result.filename);
    // The file name's own title must be the wanted title, not merely contain it.
    if title_key(&file_title(name)) != title_key(&query.title) {
        return None;
    }
    let mut notes = vec![];
    let artists = parse_artists(&query.artist);
    let artist_named = artists.main.iter().any(|a| contains_words(&folded_path, a));
    if !artist_named {
        notes.push("the artist is not named".to_string());
    }
    let wanted = classify_mix(query.mix.as_deref());
    let found = classify_mix(file_mix(name).as_deref());
    let mix = mix_fit(&wanted, &found);
    match mix {
        MixFit::Compatible => {}
        MixFit::Uncertain => notes.push("the mix may differ".into()),
        MixFit::Incompatible => notes.push("a different mix".into()),
    }
    let query_words = fold(&format!(
        "{} {} {}",
        query.artist,
        query.title,
        query.mix.as_deref().unwrap_or("")
    ));
    for (word, why) in BAD_COPY {
        if contains_words(&folded_name, word) && !contains_words(&query_words, word) {
            notes.push(why.to_string());
        }
    }
    if result.duration_ms.is_some_and(|d| d < MIN_LENGTH_MS) {
        notes.push("too short".into());
    }
    let quality = quality(result);
    if quality == Quality::Other {
        notes.push("below 320 kbps or an unlisted format".into());
    }
    Some(Assessed {
        result: result.clone(),
        quality,
        mix,
        acceptable: notes.is_empty(),
        notes,
    })
}

fn rank_key(a: &Assessed) -> (bool, MixFit, Quality, bool, u64, std::cmp::Reverse<u64>) {
    (
        !a.acceptable,
        a.mix,
        a.quality,
        !a.result.free_slot.unwrap_or(false),
        a.result.queue_length.unwrap_or(u64::MAX / 2),
        std::cmp::Reverse(a.result.upload_speed.unwrap_or(0)),
    )
}

impl PartialOrd for MixFit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MixFit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let n = |m: &MixFit| match m {
            MixFit::Compatible => 0,
            MixFit::Uncertain => 1,
            MixFit::Incompatible => 2,
        };
        n(self).cmp(&n(other))
    }
}

pub fn assess(query: &AcquisitionQuery, results: &[SearchResult]) -> Outcome {
    let mut ranked: Vec<Assessed> = results.iter().filter_map(|r| assess_one(query, r)).collect();
    ranked.sort_by_key(rank_key);
    ranked.truncate(30);
    let acceptable: Vec<&Assessed> = ranked.iter().filter(|a| a.acceptable).collect();
    let decision = if results.is_empty() {
        Decision::Nothing {
            why: "Soulseek returned no results.".into(),
        }
    } else if ranked.is_empty() {
        Decision::Nothing {
            why: "No result names this track.".into(),
        }
    } else if acceptable.is_empty() {
        Decision::Choose {
            recommended: None,
            why: "No copy meets the automatic rule (right mix, lossless or 320 kbps).".into(),
        }
    } else {
        let lengths: Vec<u64> = acceptable.iter().filter_map(|a| a.result.duration_ms).collect();
        let spread = match (lengths.iter().min(), lengths.iter().max()) {
            (Some(lo), Some(hi)) => hi - lo,
            _ => 0,
        };
        if spread > LENGTH_SPREAD_MS {
            Decision::Choose {
                recommended: Some(0),
                why: format!(
                    "Matching copies differ in length by {} s, so they may be different versions.",
                    spread / 1000
                ),
            }
        } else if lengths.is_empty() {
            Decision::Choose {
                recommended: Some(0),
                why: "No copy lists its length, so the version cannot be checked.".into(),
            }
        } else {
            Decision::Auto
        }
    };
    Outcome { ranked, decision }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(artist: &str, title: &str, mix: Option<&str>) -> AcquisitionQuery {
        AcquisitionQuery {
            artist: artist.into(),
            title: title.into(),
            mix: mix.map(str::to_string),
        }
    }

    fn r(path: &str, format: &str, kbps: Option<u32>, secs: u64) -> SearchResult {
        SearchResult {
            result_id: path.into(),
            filename: path.into(),
            size_bytes: 1,
            duration_ms: Some(secs * 1000),
            format: Some(format.into()),
            bitrate_kbps: kbps,
            ..Default::default()
        }
    }

    #[test]
    fn file_titles() {
        assert_eq!(
            file_title("01 - Alpha Unit - First Light (Extended Mix)"),
            "First Light"
        );
        assert_eq!(file_title("A1 First Light"), "First Light");
        assert_eq!(file_title("03. First Light"), "First Light");
        assert_eq!(file_title("808 State - Pacific"), "Pacific");
        assert_eq!(file_title("1979"), "1979");
        assert_eq!(file_title("Alpha Unit - Song (Part 2)"), "Song (Part 2)");
        assert_eq!(file_title("Alpha Unit - Song (Preview) [CAT001]"), "Song");
        assert_eq!(
            file_mix("Alpha Unit - Song (Dub) [CAT001]").as_deref(),
            Some("Dub")
        );
    }

    #[test]
    fn mix_fit_rules() {
        let m = |s: Option<&str>| classify_mix(s);
        assert_eq!(mix_fit(&m(None), &m(Some("Original Mix"))), MixFit::Compatible);
        assert_eq!(mix_fit(&m(Some("Original Mix")), &m(None)), MixFit::Compatible);
        assert_eq!(mix_fit(&m(Some("Dub Mix")), &m(Some("Dub"))), MixFit::Compatible);
        assert_eq!(mix_fit(&m(Some("Extended Mix")), &m(None)), MixFit::Uncertain);
        assert_eq!(mix_fit(&m(None), &m(Some("Extended Mix"))), MixFit::Uncertain);
        assert_eq!(
            mix_fit(&m(Some("Someone Remix")), &m(Some("Remix"))),
            MixFit::Uncertain
        );
        assert_eq!(mix_fit(&m(None), &m(Some("Someone Remix"))), MixFit::Incompatible);
        assert_eq!(
            mix_fit(&m(Some("A Remix")), &m(Some("B Remix"))),
            MixFit::Incompatible
        );
    }

    #[test]
    fn picks_lossless_first_and_rejects_bad_copies() {
        let query = q("Alpha Unit", "First Light", None);
        let out = assess(
            &query,
            &[
                r("@@u\\Music\\Alpha Unit - First Light.mp3", "mp3", Some(320), 300),
                r(
                    "@@v\\Alpha Unit\\EP\\01 - Alpha Unit - First Light.flac",
                    "flac",
                    None,
                    301,
                ),
                r(
                    "@@w\\Alpha Unit - First Light (Preview).mp3",
                    "mp3",
                    Some(320),
                    45,
                ),
                r(
                    "@@x\\Alpha Unit - First Light (Beta Remix).flac",
                    "flac",
                    None,
                    400,
                ),
                r("@@y\\Alpha Unit - First Light.mp3", "mp3", Some(192), 300),
                r(
                    "@@z\\Alpha Unit\\First Light\\cover.jpg.mp3",
                    "mp3",
                    Some(320),
                    300,
                ),
            ],
        );
        assert_eq!(out.decision, Decision::Auto);
        assert!(out.ranked[0].result.filename.ends_with(".flac"));
        assert!(out.ranked[0].acceptable && out.ranked[1].acceptable);
        let notes: Vec<String> = out.ranked[2..].iter().flat_map(|a| a.notes.clone()).collect();
        assert!(notes.iter().any(|n| n == "a preview"));
        assert!(notes.iter().any(|n| n == "a different mix"));
        assert!(notes.iter().any(|n| n.starts_with("below 320")));
        assert_eq!(
            out.ranked.len(),
            5,
            "the cover image path does not name the title"
        );
    }

    #[test]
    fn differing_lengths_or_versions_go_to_the_user() {
        let query = q("Alpha Unit", "First Light", Some("Extended Mix"));
        let out = assess(
            &query,
            &[
                r(
                    "@@a\\Alpha Unit - First Light (Extended Mix).flac",
                    "flac",
                    None,
                    400,
                ),
                r(
                    "@@b\\Alpha Unit - First Light (Extended Mix).mp3",
                    "mp3",
                    Some(320),
                    330,
                ),
            ],
        );
        assert!(matches!(
            out.decision,
            Decision::Choose {
                recommended: Some(0),
                ..
            }
        ));

        let unnamed = assess(
            &query,
            &[r("@@a\\Alpha Unit - First Light.flac", "flac", None, 330)],
        );
        assert!(matches!(
            unnamed.decision,
            Decision::Choose {
                recommended: None,
                ..
            }
        ));

        assert!(matches!(assess(&query, &[]).decision, Decision::Nothing { .. }));
        let unrelated = assess(&query, &[r("@@a\\Other - Song.flac", "flac", None, 300)]);
        assert!(matches!(unrelated.decision, Decision::Nothing { .. }));
    }

    #[test]
    fn peers_with_free_slots_rank_first() {
        let query = q("Alpha Unit", "First Light", None);
        let mut busy = r("@@busy\\Alpha Unit - First Light.flac", "flac", None, 300);
        busy.queue_length = Some(50);
        let mut free = r("@@free\\Alpha Unit - First Light.flac", "flac", None, 300);
        free.free_slot = Some(true);
        free.queue_length = Some(0);
        let out = assess(&query, &[busy, free]);
        assert_eq!(
            out.ranked[0].result.filename,
            "@@free\\Alpha Unit - First Light.flac"
        );
    }
}
