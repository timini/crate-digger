//! Discogs API: artist, label and release relationships.
//! https://www.discogs.com/developers
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult, CandidateProposal, EvidenceProposal};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use super::{clean_artist, split_mix};
use crate::http::{Request, Transport};

pub const API: &str = "https://api.discogs.com";

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseRef {
    pub id: u64,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub main_release: Option<u64>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
}

impl ReleaseRef {
    /// Masters point at their main release, which has the tracklist.
    pub fn release_id(&self) -> u64 {
        match self.kind.as_deref() {
            Some("master") => self.main_release.unwrap_or(self.id),
            _ => self.id,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credit {
    pub name: String,
    #[serde(default)]
    pub anv: String,
    #[serde(default)]
    pub join: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LabelCredit {
    pub name: String,
    #[serde(default)]
    pub catno: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub position: String,
    pub title: String,
    #[serde(rename = "type_", default)]
    pub kind: String,
    #[serde(default)]
    pub artists: Vec<Credit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub artists: Vec<Credit>,
    #[serde(default)]
    pub labels: Vec<LabelCredit>,
    #[serde(default)]
    pub tracklist: Vec<Track>,
}

/// Joins a Discogs credit list the way it is printed on the release.
pub fn credit_text(credits: &[Credit]) -> String {
    let mut out = String::new();
    for (i, c) in credits.iter().enumerate() {
        let name = if c.anv.is_empty() { &c.name } else { &c.anv };
        out.push_str(&clean_artist(name));
        if i + 1 < credits.len() {
            match c.join.trim() {
                "" | "," => out.push_str(", "),
                join => {
                    out.push(' ');
                    out.push_str(join);
                    out.push(' ');
                }
            }
        }
    }
    out
}

pub struct TrackFound {
    pub artist: String,
    pub title: String,
    pub mix: Option<String>,
}

impl Release {
    pub fn label(&self) -> Option<&LabelCredit> {
        self.labels.first()
    }

    /// Playable tracks with their credited artist (track artist, else release artist).
    pub fn tracks(&self) -> Vec<TrackFound> {
        self.tracklist
            .iter()
            .filter(|t| t.kind.is_empty() || t.kind == "track")
            .filter_map(|t| {
                let artists = if t.artists.is_empty() {
                    &self.artists
                } else {
                    &t.artists
                };
                let artist = credit_text(artists);
                let (title, mix) = split_mix(&t.title);
                // "Various" is a compilation placeholder, not an artist.
                (!artist.is_empty() && !title.is_empty() && !artist.eq_ignore_ascii_case("various"))
                    .then_some(TrackFound { artist, title, mix })
            })
            .collect()
    }

    pub fn page_url(&self) -> String {
        if self.uri.starts_with("https://www.discogs.com/") {
            self.uri.clone()
        } else {
            format!("https://www.discogs.com/release/{}", self.id)
        }
    }

    pub fn proposal(&self, track: &TrackFound, reason: String) -> CandidateProposal {
        let label = self.label();
        let mut excerpt = format!("{} - {}", track.artist, track.title);
        if let Some(mix) = &track.mix {
            excerpt.push_str(&format!(" ({mix})"));
        }
        excerpt.push_str(&format!(" on {}", self.title));
        if let Some(l) = label {
            excerpt.push_str(&format!(", {} {}", clean_artist(&l.name), l.catno));
        }
        if let Some(y) = self.year.filter(|y| *y > 0) {
            excerpt.push_str(&format!(", {y}"));
        }
        CandidateProposal {
            artist: track.artist.clone(),
            title: track.title.clone(),
            mix: track.mix.clone(),
            label: label.map(|l| clean_artist(&l.name)),
            release: Some(self.title.clone()),
            reasons: vec![reason],
            evidence: vec![EvidenceProposal {
                source_kind: "discogs".into(),
                source_url: Some(self.page_url()),
                supplied_text_id: None,
                excerpt: excerpt.trim().to_string(),
                confidence: 0.9,
            }],
        }
    }
}

/// A Discogs session with a request budget, so one run cannot exceed the
/// API's rate limit or run for long.
pub struct Discogs {
    token: String,
    transport: Arc<dyn Transport>,
    budget: AtomicUsize,
}

impl Discogs {
    pub fn new(token: String, transport: Arc<dyn Transport>, budget: usize) -> Self {
        Self {
            token,
            transport,
            budget: AtomicUsize::new(budget),
        }
    }

    pub fn remaining(&self) -> usize {
        self.budget.load(Ordering::SeqCst)
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> AdapterResult<Value> {
        if self
            .budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |b| b.checked_sub(1))
            .is_err()
        {
            return Err(AdapterError::RateLimited { retry_after_ms: None });
        }
        let mut url = Url::parse(API).expect("static url");
        url.set_path(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut req = Request::get(url.to_string());
        req.headers
            .push(("Authorization".into(), format!("Discogs token={}", self.token)));
        self.transport.send(req)?.json()
    }

    fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> AdapterResult<T> {
        serde_json::from_value(value)
            .map_err(|_| AdapterError::Invalid("Discogs returned an unexpected response.".into()))
    }

    /// Search results as Discogs returns them, for fields `SearchHit` leaves out.
    pub fn search_raw(&self, kind: &str, query: &[(&str, &str)]) -> AdapterResult<Vec<Value>> {
        let mut q = vec![("type", kind), ("per_page", "5")];
        q.extend_from_slice(query);
        let v = self.get("/database/search", &q)?;
        Ok(v["results"].as_array().cloned().unwrap_or_default())
    }

    pub fn search(&self, kind: &str, query: &[(&str, &str)]) -> AdapterResult<Vec<SearchHit>> {
        let mut q = vec![("type", kind), ("per_page", "5")];
        q.extend_from_slice(query);
        let v = self.get("/database/search", &q)?;
        Self::parse(v["results"].clone())
    }

    /// The search hit whose name matches `name`, ignoring case and Discogs
    /// suffixes. A near miss is not used: it would expand the wrong artist.
    pub fn find(&self, kind: &str, name: &str) -> AdapterResult<Option<SearchHit>> {
        let hits = self.search(kind, &[("q", name)])?;
        let wanted = super::fold(name);
        Ok(hits
            .iter()
            .find(|h| super::fold(&clean_artist(&h.title)) == wanted)
            .cloned())
    }

    pub fn artist_releases(&self, id: u64, per_page: usize) -> AdapterResult<Vec<ReleaseRef>> {
        let per_page = per_page.to_string();
        let v = self.get(
            &format!("/artists/{id}/releases"),
            &[("sort", "year"), ("sort_order", "desc"), ("per_page", &per_page)],
        )?;
        let refs: Vec<ReleaseRef> = Self::parse(v["releases"].clone())?;
        Ok(refs
            .into_iter()
            .filter(|r| r.role.as_deref().is_none_or(|role| role == "Main"))
            .collect())
    }

    pub fn label_releases(&self, id: u64, per_page: usize) -> AdapterResult<Vec<ReleaseRef>> {
        let per_page = per_page.to_string();
        let v = self.get(&format!("/labels/{id}/releases"), &[("per_page", &per_page)])?;
        let mut refs: Vec<ReleaseRef> = Self::parse(v["releases"].clone())?;
        refs.sort_by_key(|r| std::cmp::Reverse(r.year.unwrap_or(0)));
        Ok(refs)
    }

    pub fn release(&self, id: u64) -> AdapterResult<Release> {
        Self::parse(self.get(&format!("/releases/{id}"), &[])?)
    }
}
