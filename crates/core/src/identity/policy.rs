//! The identity policy: given what is known about two tracks, are they the
//! same recording, different versions of one work, or unrelated?
//!
//! Rules from the spec:
//! - Fingerprints identify audio. A fingerprint match, a recording-level
//!   identifier (ISRC, MusicBrainz recording) or the user's confirmation is
//!   required before two tracks are treated as the same recording.
//! - Title similarity, embedding proximity or an LLM's assertion never merge
//!   anything on their own.
//! - Conflicting evidence goes to review with its provenance.
//! - Unknown stays unknown.

use serde::{Deserialize, Serialize};

use super::normalize::{classify_mix, parse_artists, title_key, MixClass};

/// Identifier namespaces that name a single recording.
pub const RECORDING_NAMESPACES: &[&str] = &["isrc", "musicbrainz_recording"];

/// What is known about one side of a comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Side {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub mix: Option<String>,
    pub duration_ms: Option<i64>,
    /// (namespace, value)
    pub external_ids: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// Byte-identical files.
    IdenticalBytes,
    /// Chromaprint alignment. `coverage` is the share of the shorter
    /// recording that aligned; `speed` is the playback speed of B relative
    /// to A at which it aligned (1.0 = not pitched).
    FingerprintMatch { score: f64, coverage: f64, speed: f64 },
    /// Both were fingerprinted and did not align.
    FingerprintMismatch { score: f64 },
    /// Similar sound. Never proof of identity.
    EmbeddingSimilarity { similarity: f64, model: String },
    /// A language model said they are the same. Never proof of identity.
    LlmAssertion { claim: String },
    /// The user decided.
    UserDecision { relation: Relation },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    SameRecording,
    DifferentVersion,
    Unrelated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    SameRecording,
    /// The same recording played faster or slower, by this percentage.
    PitchedCopy {
        percent: f64,
    },
    /// Versions of the same work (remix, edit, dub...).
    DifferentVersion,
    Unrelated,
    /// Not enough evidence either way.
    Unknown,
    /// The evidence conflicts; a person has to decide.
    NeedsReview {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub verdict: Verdict,
    /// Whether the verdict may be applied without asking. Merges are only
    /// automatic with audio or identifier proof; linking versions is always
    /// safe because nothing is combined.
    pub automatic: bool,
    /// Plain-language reasons, for the evidence record and the UI.
    pub reasons: Vec<String>,
}

/// Thresholds for fingerprint evidence, calibrated on the labelled fixtures
/// (see docs/identity-calibration.md).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// Minimum alignment score for a fingerprint match.
    pub match_score: f64,
    /// Coverage above which aligned audio is the whole recording rather than
    /// a section or edit.
    pub full_coverage: f64,
    /// Allowed difference between duration ratio and speed for a pitched copy.
    pub pitch_tolerance: f64,
    /// Allowed duration difference for "the same length", in milliseconds.
    pub duration_tolerance_ms: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            match_score: 0.6,
            full_coverage: 0.85,
            pitch_tolerance: 0.015,
            duration_tolerance_ms: 3_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meta {
    /// Same artist, title and compatible mix.
    SameRecordingTags,
    /// Same artist and title, different mix.
    OtherVersionTags,
    /// Titles or artists differ.
    DifferentTags,
    /// Something needed to compare is missing.
    Insufficient,
}

fn compare_tags(a: &Side, b: &Side) -> (Meta, MixClass, MixClass) {
    let (ma, mb) = (classify_mix(a.mix.as_deref()), classify_mix(b.mix.as_deref()));
    let (Some(aa), Some(ba), Some(at), Some(bt)) = (&a.artist, &b.artist, &a.title, &b.title) else {
        return (Meta::Insufficient, ma, mb);
    };
    let same_artist = parse_artists(aa).overlaps(&parse_artists(ba));
    let same_title = title_key(at) == title_key(bt);
    let meta = if !(same_artist && same_title) {
        Meta::DifferentTags
    } else if ma.same_recording_as(&mb) {
        Meta::SameRecordingTags
    } else {
        Meta::OtherVersionTags
    };
    (meta, ma, mb)
}

fn recording_ids(side: &Side) -> impl Iterator<Item = &(String, String)> {
    side.external_ids
        .iter()
        .filter(|(ns, _)| RECORDING_NAMESPACES.contains(&ns.as_str()))
}

/// Compare recording-level identifiers: Some(true) if they share one,
/// Some(false) if both have one in the same namespace and they differ.
fn compare_ids(a: &Side, b: &Side) -> Option<bool> {
    let mut conflict = false;
    for (ns, va) in recording_ids(a) {
        for (nb, vb) in recording_ids(b) {
            if ns == nb {
                if va.eq_ignore_ascii_case(vb) {
                    return Some(true);
                }
                conflict = true;
            }
        }
    }
    conflict.then_some(false)
}

fn describe_mix(m: &MixClass) -> String {
    match m {
        MixClass::Unspecified => "no mix named".into(),
        MixClass::Remix(Some(by)) => format!("{by} remix"),
        MixClass::Edit(Some(by)) => format!("{by} edit"),
        MixClass::Other(s) => s.clone(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn decision(verdict: Verdict, automatic: bool, reasons: Vec<String>) -> Decision {
    Decision {
        verdict,
        automatic,
        reasons,
    }
}

pub fn decide(a: &Side, b: &Side, evidence: &[Evidence], t: &Thresholds) -> Decision {
    // The user's decision stands.
    if let Some(r) = evidence.iter().rev().find_map(|e| match e {
        Evidence::UserDecision { relation } => Some(*relation),
        _ => None,
    }) {
        let v = match r {
            Relation::SameRecording => Verdict::SameRecording,
            Relation::DifferentVersion => Verdict::DifferentVersion,
            Relation::Unrelated => Verdict::Unrelated,
        };
        return decision(v, true, vec!["You decided this.".into()]);
    }
    if evidence.contains(&Evidence::IdenticalBytes) {
        return decision(
            Verdict::SameRecording,
            true,
            vec!["The files are byte-for-byte identical.".into()],
        );
    }

    let (meta, ma, mb) = compare_tags(a, b);
    let ids = compare_ids(a, b);
    let mut reasons = Vec::new();
    for e in evidence {
        match e {
            Evidence::EmbeddingSimilarity { similarity, model } => reasons.push(format!(
                "They sound similar ({:.0}% with {model}), which alone does not show they are the same recording.",
                similarity * 100.0
            )),
            Evidence::LlmAssertion { claim } => reasons.push(format!(
                "An AI suggestion says \"{claim}\", which is not treated as proof."
            )),
            _ => {}
        }
    }

    let fp_match = evidence.iter().find_map(|e| match e {
        Evidence::FingerprintMatch {
            score,
            coverage,
            speed,
        } if *score >= t.match_score => Some((*score, *coverage, *speed)),
        _ => None,
    });
    let fp_mismatch = evidence.iter().any(|e| match e {
        Evidence::FingerprintMismatch { .. } => true,
        Evidence::FingerprintMatch { score, .. } => *score < t.match_score,
        _ => false,
    });

    if let Some((score, coverage, speed)) = fp_match {
        let pitched = (speed - 1.0).abs() > 0.005;
        reasons.push(if pitched {
            format!(
                "The audio matches when played {:+.1}% faster (alignment {:.0}%).",
                (speed - 1.0) * 100.0,
                score * 100.0
            )
        } else {
            format!(
                "The audio fingerprints match over {:.0}% of the shorter track.",
                coverage * 100.0
            )
        });
        if ids == Some(false) {
            reasons.push("But their recording identifiers (ISRC) differ.".into());
            return decision(
                Verdict::NeedsReview {
                    reason: "The audio matches but the recording identifiers differ.".into(),
                },
                false,
                reasons,
            );
        }
        match meta {
            Meta::DifferentTags => {
                reasons.push("But the artist or title differs; one file may be mislabelled.".into());
                return decision(
                    Verdict::NeedsReview {
                        reason: "The audio matches but the artist or title differs.".into(),
                    },
                    false,
                    reasons,
                );
            }
            Meta::OtherVersionTags if coverage >= t.full_coverage => {
                reasons.push(format!(
                    "But the tags name different mixes ({} and {}).",
                    describe_mix(&ma),
                    describe_mix(&mb)
                ));
                return decision(
                    Verdict::NeedsReview {
                        reason: "The audio matches but the tags name different mixes.".into(),
                    },
                    false,
                    reasons,
                );
            }
            _ => {}
        }
        if coverage < t.full_coverage {
            // Shares audio but one is shorter: an edit or section.
            reasons.push(
                "Only part of the audio is shared, so this is an edit or excerpt of the same work.".into(),
            );
            return decision(Verdict::DifferentVersion, true, reasons);
        }
        if pitched {
            let ratio_ok = match (a.duration_ms, b.duration_ms) {
                (Some(da), Some(db)) if db > 0 => {
                    let expected = da as f64 / speed;
                    ((db as f64 - expected) / expected).abs() <= t.pitch_tolerance
                }
                _ => false,
            };
            if !ratio_ok {
                reasons.push("But the lengths do not agree with the speed change.".into());
                return decision(
                    Verdict::NeedsReview {
                        reason: "The audio matches at a different speed but the lengths do not fit.".into(),
                    },
                    false,
                    reasons,
                );
            }
            reasons.push("The lengths agree with the speed change.".into());
            return decision(
                Verdict::PitchedCopy {
                    percent: ((speed - 1.0) * 1000.0).round() / 10.0,
                },
                true,
                reasons,
            );
        }
        return decision(Verdict::SameRecording, true, reasons);
    }

    if ids == Some(true) {
        reasons.push("They share a recording identifier (ISRC or MusicBrainz).".into());
        if meta == Meta::OtherVersionTags || meta == Meta::DifferentTags {
            return decision(
                Verdict::NeedsReview {
                    reason: "They share a recording identifier but their tags disagree.".into(),
                },
                false,
                reasons,
            );
        }
        if fp_mismatch {
            return decision(
                Verdict::NeedsReview {
                    reason: "They share a recording identifier but the audio differs.".into(),
                },
                false,
                reasons,
            );
        }
        return decision(Verdict::SameRecording, true, reasons);
    }

    match meta {
        Meta::OtherVersionTags => {
            reasons.push(format!(
                "Same artist and title, different mixes ({} and {}).",
                describe_mix(&ma),
                describe_mix(&mb)
            ));
            decision(Verdict::DifferentVersion, true, reasons)
        }
        Meta::SameRecordingTags if fp_mismatch => {
            // Tags agree but the audio does not: a mislabelled file, a
            // different version, or a different master that changed a lot.
            let lengths_differ = match (a.duration_ms, b.duration_ms) {
                (Some(x), Some(y)) => (x - y).abs() > t.duration_tolerance_ms,
                _ => false,
            };
            reasons.push("The tags match but the audio fingerprints do not.".into());
            if lengths_differ {
                reasons.push("Their lengths also differ.".into());
            }
            decision(
                Verdict::NeedsReview {
                    reason: "The tags match but the audio is different.".into(),
                },
                false,
                reasons,
            )
        }
        Meta::SameRecordingTags => {
            reasons.push(
                "Same artist, title and mix, but matching names do not prove it is the same recording."
                    .into(),
            );
            decision(Verdict::Unknown, false, reasons)
        }
        Meta::DifferentTags => {
            reasons.push("Different artist or title.".into());
            decision(Verdict::Unrelated, false, reasons)
        }
        Meta::Insufficient => {
            reasons.push("Not enough information to compare.".into());
            decision(Verdict::Unknown, false, reasons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(artist: &str, title: &str, mix: Option<&str>, secs: i64) -> Side {
        Side {
            artist: Some(artist.into()),
            title: Some(title.into()),
            mix: mix.map(str::to_string),
            duration_ms: Some(secs * 1000),
            external_ids: vec![],
        }
    }

    fn fp(score: f64, coverage: f64, speed: f64) -> Evidence {
        Evidence::FingerprintMatch {
            score,
            coverage,
            speed,
        }
    }

    fn d(a: &Side, b: &Side, e: &[Evidence]) -> Decision {
        decide(a, b, e, &Thresholds::default())
    }

    #[test]
    fn fingerprint_match_with_compatible_tags_is_the_same_recording() {
        let a = side("Kerri Chandler", "Rain", Some("Original Mix"), 420);
        let b = side("kerri chandler", "Rain", None, 419);
        let r = d(&a, &b, &[fp(0.92, 0.98, 1.0)]);
        assert_eq!(r.verdict, Verdict::SameRecording);
        assert!(r.automatic);
    }

    #[test]
    fn a_remaster_is_the_same_recording() {
        let a = side("A", "T", None, 300);
        let b = side("A", "T", Some("2019 Remaster"), 300);
        assert_eq!(d(&a, &b, &[fp(0.8, 0.95, 1.0)]).verdict, Verdict::SameRecording);
    }

    #[test]
    fn title_similarity_alone_never_proves_identity() {
        let a = side("A", "T", Some("Original Mix"), 300);
        let b = side("A", "T", Some("Original Mix"), 300);
        let r = d(&a, &b, &[]);
        assert_eq!(r.verdict, Verdict::Unknown);
        assert!(!r.automatic);
    }

    #[test]
    fn embedding_and_llm_evidence_never_merge() {
        let a = side("A", "T", None, 300);
        let b = side("A", "T", None, 300);
        let e = [
            Evidence::EmbeddingSimilarity {
                similarity: 0.999,
                model: "m".into(),
            },
            Evidence::LlmAssertion {
                claim: "these are the same track".into(),
            },
        ];
        let r = d(&a, &b, &e);
        assert_eq!(r.verdict, Verdict::Unknown);
        assert!(!r.automatic);
        assert!(r.reasons.iter().any(|x| x.contains("not treated as proof")));
        // Even with different titles, similar sound says nothing.
        let c = side("B", "Other", None, 300);
        assert_eq!(d(&a, &c, &e).verdict, Verdict::Unrelated);
    }

    #[test]
    fn different_mixes_are_different_versions_not_merged() {
        let a = side("A", "T", Some("Original Mix"), 400);
        let b = side("A", "T", Some("Kerri Chandler Remix"), 460);
        let r = d(&a, &b, &[]);
        assert_eq!(r.verdict, Verdict::DifferentVersion);
        let r = d(&a, &b, &[Evidence::FingerprintMismatch { score: 0.1 }]);
        assert_eq!(r.verdict, Verdict::DifferentVersion);
    }

    #[test]
    fn same_audio_with_different_mix_tags_needs_review() {
        let a = side("A", "T", Some("Extended Mix"), 400);
        let b = side("A", "T", Some("Original Mix"), 400);
        let r = d(&a, &b, &[fp(0.9, 0.97, 1.0)]);
        assert!(matches!(r.verdict, Verdict::NeedsReview { .. }));
        assert!(!r.automatic);
    }

    #[test]
    fn same_audio_with_different_titles_needs_review() {
        let a = side("A", "Track One", None, 400);
        let b = side("A", "Totally Different", None, 400);
        assert!(matches!(
            d(&a, &b, &[fp(0.9, 0.97, 1.0)]).verdict,
            Verdict::NeedsReview { .. }
        ));
    }

    #[test]
    fn matching_tags_with_different_audio_needs_review() {
        let a = side("A", "T", None, 400);
        let b = side("A", "T", None, 250);
        let r = d(&a, &b, &[Evidence::FingerprintMismatch { score: 0.05 }]);
        assert!(matches!(r.verdict, Verdict::NeedsReview { .. }));
        assert!(r.reasons.iter().any(|x| x.contains("lengths also differ")));
    }

    #[test]
    fn partial_audio_overlap_is_an_edit_of_the_same_work() {
        let a = side("A", "T", Some("Extended Mix"), 420);
        let b = side("A", "T", Some("Radio Edit"), 210);
        let r = d(&a, &b, &[fp(0.85, 0.5, 1.0)]);
        assert_eq!(r.verdict, Verdict::DifferentVersion);
        assert!(r.automatic, "linking versions is safe");
    }

    #[test]
    fn pitched_copies_need_lengths_that_fit_the_speed() {
        let a = side("A", "T", None, 400);
        // Played 4% faster: 400 / 1.04 = 384.6 s.
        let b = side("A", "T", None, 385);
        let r = d(&a, &b, &[fp(0.8, 0.95, 1.04)]);
        assert_eq!(r.verdict, Verdict::PitchedCopy { percent: 4.0 });
        assert!(r.automatic);
        let wrong_length = side("A", "T", None, 330);
        assert!(matches!(
            d(&a, &wrong_length, &[fp(0.8, 0.95, 1.04)]).verdict,
            Verdict::NeedsReview { .. }
        ));
    }

    #[test]
    fn weak_fingerprint_alignment_is_not_a_match() {
        let a = side("A", "T", None, 400);
        let b = side("A", "T", None, 400);
        let r = d(&a, &b, &[fp(0.2, 0.9, 1.0)]);
        assert!(matches!(r.verdict, Verdict::NeedsReview { .. }), "{r:?}");
    }

    #[test]
    fn recording_identifiers() {
        let mut a = side("A", "T", None, 400);
        let mut b = side("A", "T", None, 400);
        a.external_ids.push(("isrc".into(), "GBAAA0000001".into()));
        b.external_ids.push(("isrc".into(), "gbaaa0000001".into()));
        assert_eq!(d(&a, &b, &[]).verdict, Verdict::SameRecording);
        // A release-level Discogs ID is not recording identity.
        let mut c = side("A", "T", None, 400);
        let mut e = side("A", "T", None, 400);
        c.external_ids.push(("discogs_release".into(), "1".into()));
        e.external_ids.push(("discogs_release".into(), "1".into()));
        assert_eq!(d(&c, &e, &[]).verdict, Verdict::Unknown);
        // Different ISRCs with matching audio go to review.
        b.external_ids = vec![("isrc".into(), "GBAAA0000002".into())];
        assert!(matches!(
            d(&a, &b, &[fp(0.9, 0.99, 1.0)]).verdict,
            Verdict::NeedsReview { .. }
        ));
    }

    #[test]
    fn missing_information_stays_unknown() {
        let a = Side::default();
        let b = side("A", "T", None, 400);
        assert_eq!(d(&a, &b, &[]).verdict, Verdict::Unknown);
    }

    #[test]
    fn user_decision_and_identical_bytes_win() {
        let a = side("A", "T", None, 400);
        let b = side("B", "U", None, 100);
        let r = d(
            &a,
            &b,
            &[Evidence::UserDecision {
                relation: Relation::SameRecording,
            }],
        );
        assert_eq!(r.verdict, Verdict::SameRecording);
        assert_eq!(
            d(&a, &b, &[Evidence::IdenticalBytes]).verdict,
            Verdict::SameRecording
        );
    }
}
