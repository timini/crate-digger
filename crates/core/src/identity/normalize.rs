//! Comparable forms of artist, title and mix names.
//!
//! These keys are for matching only; what the user sees is never altered.

use serde::{Deserialize, Serialize};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Lower case, accents removed, punctuation turned into spaces, spaces
/// collapsed. "Róisín Murphy!" becomes "roisin murphy".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd().filter(|c| !is_combining_mark(*c)) {
        for l in c.to_lowercase() {
            if l.is_alphanumeric() {
                out.push(l);
            } else if l == '\'' || l == '’' {
                // "Don't" and "Dont" should match.
            } else {
                out.push(' ');
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Who a track is credited to, as folded names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistCredit {
    pub main: Vec<String>,
    pub featured: Vec<String>,
}

impl ArtistCredit {
    /// Whether two credits name at least one main artist in common. Order
    /// and featured guests are ignored.
    pub fn overlaps(&self, other: &ArtistCredit) -> bool {
        self.main.iter().any(|a| other.main.contains(a))
    }
}

const FEAT_MARKERS: &[&str] = &[" featuring ", " feat. ", " feat ", " ft. ", " ft ", " with "];
const SEPARATORS: &[&str] = &[" & ", " and ", " x ", " vs. ", " vs ", ", ", " + ", " / "];

fn split_all(s: &str, seps: &[&str]) -> Vec<String> {
    let mut parts = vec![s.to_string()];
    for sep in seps {
        parts = parts
            .into_iter()
            .flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>())
            .collect();
    }
    parts
}

/// Parse "A & B feat. C" into main artists [a, b] and featured [c].
pub fn parse_artists(artist: &str) -> ArtistCredit {
    // Lower-case before splitting so markers match in any case; keep
    // punctuation for now because "feat." needs its dot.
    let lower = format!(" {} ", artist.to_lowercase().replace(['(', ')', '[', ']'], " "));
    let (main_part, feat_part) = FEAT_MARKERS
        .iter()
        .filter_map(|m| lower.find(m).map(|i| (i, m.len())))
        .min_by_key(|(i, _)| *i)
        .map(|(i, len)| (lower[..i].to_string(), Some(lower[i + len..].to_string())))
        .unwrap_or((lower.clone(), None));
    let names = |s: &str| -> Vec<String> {
        let mut v: Vec<String> = split_all(&format!(" {} ", s.trim()), SEPARATORS)
            .iter()
            .map(|p| fold(p))
            .filter(|p| !p.is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    ArtistCredit {
        main: names(&main_part),
        featured: feat_part.map(|f| names(&f)).unwrap_or_default(),
    }
}

/// A title for matching: folded, with any "(feat. X)" removed.
pub fn title_key(title: &str) -> String {
    let lower = title.to_lowercase();
    let cut = ["(feat", "[feat", "(ft.", "[ft.", " feat. ", " ft. "]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        .unwrap_or(lower.len());
    fold(&title[..cut.min(title.len())])
}

/// What kind of version a mix name describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "by", rename_all = "snake_case")]
pub enum MixClass {
    /// No mix named. Usually the original, but not certain.
    Unspecified,
    Original,
    Extended,
    RadioEdit,
    Club,
    Dub,
    Instrumental,
    Acapella,
    Live,
    Vip,
    /// Same recording, new master ("2012 Remaster").
    Remaster,
    /// A remix, rework, refix, bootleg or flip, by whom if known.
    Remix(Option<String>),
    /// An edit by someone, which changes the arrangement.
    Edit(Option<String>),
    Other(String),
}

impl MixClass {
    /// Whether two mix names can describe the same recording. Unspecified
    /// and remaster count as the original.
    pub fn same_recording_as(&self, other: &MixClass) -> bool {
        use MixClass::*;
        let base = |m: &MixClass| match m {
            Unspecified | Original | Remaster => Original,
            other => other.clone(),
        };
        base(self) == base(other)
    }

    /// Whether this names a specific version rather than the original.
    pub fn is_specific(&self) -> bool {
        !matches!(
            self,
            MixClass::Unspecified | MixClass::Original | MixClass::Remaster
        )
    }
}

fn strip_suffix_word<'a>(s: &'a str, words: &[&str]) -> Option<&'a str> {
    words.iter().find_map(|w| {
        s.strip_suffix(w)
            .map(str::trim)
            .filter(|rest| rest.is_empty() || s.ends_with(&format!(" {w}")))
    })
}

/// Classify a mix name such as "Kerri Chandler Remix" or "Radio Edit".
pub fn classify_mix(mix: Option<&str>) -> MixClass {
    let Some(raw) = mix.map(str::trim).filter(|m| !m.is_empty()) else {
        return MixClass::Unspecified;
    };
    let m = fold(raw);
    let by = |rest: &str| -> Option<String> {
        let r = rest.trim();
        (!r.is_empty()).then(|| r.to_string())
    };
    match m.as_str() {
        "original" | "original mix" | "original version" | "album version" | "main mix" => {
            return MixClass::Original
        }
        "extended" | "extended mix" | "extended version" | "extended original mix" => {
            return MixClass::Extended
        }
        "radio edit" | "radio mix" | "radio version" | "single edit" | "single version" => {
            return MixClass::RadioEdit
        }
        "club mix" | "club version" => return MixClass::Club,
        "dub" | "dub mix" | "dub version" => return MixClass::Dub,
        "instrumental" | "instrumental mix" | "instrumental version" => return MixClass::Instrumental,
        "acapella" | "a cappella" | "acappella" => return MixClass::Acapella,
        "live" | "live version" => return MixClass::Live,
        "vip" | "vip mix" => return MixClass::Vip,
        _ => {}
    }
    if m.contains("remaster") {
        return MixClass::Remaster;
    }
    if let Some(rest) = strip_suffix_word(
        &m,
        &["remix", "rework", "refix", "bootleg", "flip", "re edit", "reedit"],
    ) {
        return MixClass::Remix(by(rest));
    }
    if let Some(rest) = strip_suffix_word(&m, &["edit"]) {
        return MixClass::Edit(by(rest));
    }
    if let Some(rest) = strip_suffix_word(&m, &["dub"]) {
        // "Kerri Chandler Dub" is a remixer's dub.
        return MixClass::Remix(by(&format!("{rest} dub")));
    }
    if let Some(rest) = strip_suffix_word(&m, &["mix", "version"]) {
        return MixClass::Remix(by(rest));
    }
    MixClass::Other(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_removes_case_accents_and_punctuation() {
        assert_eq!(fold("Róisín Murphy!"), "roisin murphy");
        assert_eq!(fold("  AC/DC  "), "ac dc");
        assert_eq!(fold("Don't Stop"), "dont stop");
        assert_eq!(fold("Beyoncé"), fold("BEYONCE"));
        assert_eq!(fold("ＦＵＬＬ　ＷＩＤＴＨ"), "full width");
    }

    #[test]
    fn artist_credits_split_main_and_featured() {
        let c = parse_artists("Disclosure & Sam Smith feat. Lorde");
        assert_eq!(c.main, vec!["disclosure", "sam smith"]);
        assert_eq!(c.featured, vec!["lorde"]);
        assert!(parse_artists("Sam Smith and Disclosure").overlaps(&c));
        assert_eq!(
            parse_artists("Fred again.. x Skrillex").main,
            vec!["fred again", "skrillex"]
        );
        assert_eq!(
            parse_artists("DJ Koze (feat. Róisín Murphy)").featured,
            vec!["roisin murphy"]
        );
        assert!(!parse_artists("Moodymann").overlaps(&parse_artists("Theo Parrish")));
    }

    #[test]
    fn title_keys_ignore_featured_artists() {
        assert_eq!(title_key("Latch (feat. Sam Smith)"), "latch");
        assert_eq!(title_key("Latch"), "latch");
        assert_eq!(title_key("Strings Of Life"), title_key("strings of life"));
    }

    #[test]
    fn mix_names_are_classified() {
        use MixClass::*;
        assert_eq!(classify_mix(None), Unspecified);
        assert_eq!(classify_mix(Some("Original Mix")), Original);
        assert_eq!(classify_mix(Some("Extended Mix")), Extended);
        assert_eq!(classify_mix(Some("Radio Edit")), RadioEdit);
        assert_eq!(classify_mix(Some("Dub")), Dub);
        assert_eq!(classify_mix(Some("2012 Remaster")), Remaster);
        assert_eq!(
            classify_mix(Some("Kerri Chandler Remix")),
            Remix(Some("kerri chandler".into()))
        );
        assert_eq!(
            classify_mix(Some("Moodymann Edit")),
            Edit(Some("moodymann".into()))
        );
        assert_eq!(
            classify_mix(Some("Joe Claussell Dub")),
            Remix(Some("joe claussell dub".into()))
        );
        assert_eq!(
            classify_mix(Some("Larry Heard Mix")),
            Remix(Some("larry heard".into()))
        );
        assert_eq!(classify_mix(Some("VIP")), Vip);
        assert_eq!(classify_mix(Some("Part 2")), Other("part 2".into()));
    }

    #[test]
    fn original_unspecified_and_remaster_are_the_same_recording() {
        use MixClass::*;
        assert!(Unspecified.same_recording_as(&Original));
        assert!(Remaster.same_recording_as(&Unspecified));
        assert!(!Original.same_recording_as(&Extended));
        assert!(!Remix(Some("a".into())).same_recording_as(&Remix(Some("b".into()))));
        assert!(Remix(Some("a".into())).same_recording_as(&Remix(Some("a".into()))));
    }
}
