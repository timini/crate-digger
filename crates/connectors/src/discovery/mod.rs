//! Tier 1 discovery from Discogs, public pages, pasted text and model
//! suggestions. Only candidates backed by something retrievable are
//! verified; model suggestions without it stay unverified.
pub mod discogs;
pub mod extract;
pub mod pages;
pub mod suggest;

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use cd_core::adapters::{
    AdapterError, AdapterResult, CandidateProposal, DiscoveryInput, DiscoveryRequest, DiscoverySource, Seed,
};
use cd_core::domain::SeedKind;
pub use cd_core::identity::normalize::fold;

use crate::config::Connections;
use crate::credentials::{Credential, SecretStore};
use crate::http::Transport;
use crate::llm::LlmClient;
use discogs::{Discogs, Release};

/// Discogs allows 60 authenticated requests a minute; one run stays well under that.
pub const DISCOGS_BUDGET: usize = 40;

/// Strips Discogs disambiguation ("Name (2)") and variation marks ("Name*").
pub fn clean_artist(name: &str) -> String {
    let mut s = name.trim().trim_end_matches('*').trim().to_string();
    if let Some(open) = s.rfind(" (") {
        let inner = &s[open + 2..];
        if inner.ends_with(')') && inner[..inner.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            s.truncate(open);
        }
    }
    s
}

const MIX_WORDS: &[&str] = &[
    "mix",
    "remix",
    "edit",
    "dub",
    "version",
    "vip",
    "rework",
    "instrumental",
    "extended",
    "radio",
    "club",
    "bootleg",
    "refix",
    "remaster",
    "original",
];

/// "Title (Dub Mix)" becomes ("Title", Some("Dub Mix")). Brackets that do not
/// name a mix, such as "(Part 1)", stay in the title.
pub fn split_mix(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let (Some(start), true) = (t.rfind(open), t.ends_with(close)) {
            let inner = t[start + 1..t.len() - 1].trim();
            let words = fold(inner);
            if start > 0 && words.split(' ').any(|w| MIX_WORDS.contains(&w)) {
                return (t[..start].trim().to_string(), Some(inner.to_string()));
            }
        }
    }
    (t.to_string(), None)
}

fn key(p: &CandidateProposal) -> (String, String, String) {
    (
        fold(&p.artist),
        fold(&p.title),
        fold(p.mix.as_deref().unwrap_or("")),
    )
}

/// Collects proposals, merging repeats so each keeps all its evidence.
#[derive(Default)]
pub struct Collector {
    pub proposals: Vec<CandidateProposal>,
}

impl Collector {
    pub fn add(&mut self, p: CandidateProposal) {
        match self.proposals.iter_mut().find(|q| key(q) == key(&p)) {
            Some(existing) => {
                for r in p.reasons {
                    if !existing.reasons.contains(&r) {
                        existing.reasons.push(r);
                    }
                }
                existing.evidence.extend(p.evidence);
            }
            None => self.proposals.push(p),
        }
    }

    pub fn len(&self) -> usize {
        self.proposals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }
}

/// Seed expansion through Discogs relationships. Stops quietly when the
/// request budget runs out; returns an error only if nothing was found.
pub fn expand(discogs: &Discogs, seeds: &[Seed], limit: usize, out: &mut Collector) -> AdapterResult<()> {
    let mut seen_releases = HashSet::new();
    let mut first_error = None;
    for seed in seeds {
        if out.len() >= limit || discogs.remaining() < 2 {
            break;
        }
        let result = match seed.kind {
            SeedKind::Artist | SeedKind::Dj => expand_artist(discogs, seed, limit, &mut seen_releases, out),
            SeedKind::Label => expand_label(discogs, &seed.value, None, limit, &mut seen_releases, out),
            SeedKind::Track => expand_track(discogs, &seed.value, limit, &mut seen_releases, out),
        };
        match result {
            Ok(()) => {}
            // Rejected credentials stop the run so the connector pauses for the user.
            Err(e @ AdapterError::Auth(_)) => return Err(e),
            Err(AdapterError::RateLimited { .. }) if !out.is_empty() => break,
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match first_error {
        Some(e) if out.is_empty() => Err(e),
        _ => Ok(()),
    }
}

fn add_release(
    discogs: &Discogs,
    id: u64,
    seen: &mut HashSet<u64>,
    reason: &dyn Fn(&Release) -> String,
    only_artist: Option<&str>,
    limit: usize,
    out: &mut Collector,
) -> AdapterResult<Option<Release>> {
    if !seen.insert(id) || out.len() >= limit {
        return Ok(None);
    }
    let release = discogs.release(id)?;
    let reason = reason(&release);
    for track in release.tracks() {
        if out.len() >= limit {
            break;
        }
        if let Some(artist) = only_artist {
            let credit = cd_core::identity::normalize::parse_artists(&track.artist);
            if !credit.overlaps(&cd_core::identity::normalize::parse_artists(artist)) {
                continue;
            }
        }
        out.add(release.proposal(&track, reason.clone()));
    }
    Ok(Some(release))
}

fn expand_artist(
    discogs: &Discogs,
    seed: &Seed,
    limit: usize,
    seen: &mut HashSet<u64>,
    out: &mut Collector,
) -> AdapterResult<()> {
    let Some(artist) = discogs.find("artist", &seed.value)? else {
        return Ok(());
    };
    let name = clean_artist(&artist.title);
    let what = if seed.kind == SeedKind::Dj {
        "one of your DJs"
    } else {
        "one of your seeds"
    };
    let mut labels = vec![];
    for r in discogs.artist_releases(artist.id, 10)?.iter().take(3) {
        let reason = |_: &Release| format!("Released by {name}, {what}");
        if let Some(release) = add_release(discogs, r.release_id(), seen, &reason, Some(&name), limit, out)? {
            if let Some(label) = release.label() {
                let label = clean_artist(&label.name);
                if !labels.contains(&label) && !fold(&label).contains("not on label") {
                    labels.push(label);
                }
            }
        }
    }
    // Labels that release the seed artist lead to other artists.
    for label in labels.iter().take(1) {
        if out.len() >= limit || discogs.remaining() < 3 {
            break;
        }
        expand_label(discogs, label, Some(&name), limit, seen, out)?;
    }
    Ok(())
}

fn expand_label(
    discogs: &Discogs,
    label: &str,
    via_artist: Option<&str>,
    limit: usize,
    seen: &mut HashSet<u64>,
    out: &mut Collector,
) -> AdapterResult<()> {
    let Some(hit) = discogs.find("label", label)? else {
        return Ok(());
    };
    let name = clean_artist(&hit.title);
    let reason = |_: &Release| match via_artist {
        Some(artist) if fold(artist) == fold(&name) => format!("On {name}'s own label"),
        Some(artist) => format!("On {name}, which also releases {artist}"),
        None => format!("On {name}, a label in your seeds"),
    };
    let take = if via_artist.is_some() { 2 } else { 4 };
    let mut taken = 0;
    for r in discogs.label_releases(hit.id, 30)? {
        if taken >= take || out.len() >= limit || discogs.remaining() == 0 {
            break;
        }
        // Following a label from an artist seed should find other artists.
        if let (Some(artist), Some(credited)) = (via_artist, &r.artist) {
            if fold(credited).contains(&fold(artist)) {
                continue;
            }
        }
        if add_release(discogs, r.release_id(), seen, &reason, None, limit, out)?.is_some() {
            taken += 1;
        }
    }
    Ok(())
}

fn expand_track(
    discogs: &Discogs,
    value: &str,
    limit: usize,
    seen: &mut HashSet<u64>,
    out: &mut Collector,
) -> AdapterResult<()> {
    let Some((artist, title)) = value.split_once(" - ").or_else(|| value.split_once(" – ")) else {
        return Err(AdapterError::Invalid(
            "Write track seeds as Artist - Title.".into(),
        ));
    };
    let (title, _) = split_mix(title);
    let hits = discogs.search("release", &[("artist", artist.trim()), ("track", title.trim())])?;
    let Some(hit) = hits.first() else {
        return Ok(());
    };
    let release = discogs.release(hit.id)?;
    seen.insert(release.id);
    if let Some(label) = release.label() {
        let label = clean_artist(&label.name);
        expand_label(discogs, &label, Some(artist.trim()), limit, seen, out)?;
    }
    Ok(())
}

/// The live discovery source. Configuration is shared with Settings so a
/// change applies to the next run without restarting.
pub struct LiveSource {
    /// `LIVE_SOURCE` for seeds, `PAGE_SOURCE` for pages and pasted text. Two
    /// names keep a rejected Discogs token from pausing pasted-text runs.
    pub name: &'static str,
    pub config: Arc<RwLock<Connections>>,
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Arc<dyn Transport>,
}

pub const LIVE_SOURCE: &str = "live";
pub const PAGE_SOURCE: &str = "pages";

impl LiveSource {
    fn model(&self) -> Option<Box<dyn LlmClient>> {
        let config = self.config.read().unwrap().clone();
        if config.llm_model.trim().is_empty() {
            return None;
        }
        crate::llm::client(&config, &*self.secrets, self.transport.clone()).ok()
    }

    fn discogs(&self) -> AdapterResult<Option<Discogs>> {
        Ok(self
            .secrets
            .get(Credential::Discogs)?
            .map(|token| Discogs::new(token, self.transport.clone(), DISCOGS_BUDGET)))
    }
}

impl DiscoverySource for LiveSource {
    fn id(&self) -> &str {
        self.name
    }

    fn discover(&self, request: &DiscoveryRequest) -> AdapterResult<Vec<CandidateProposal>> {
        let model = self.model();
        let mut out = Collector::default();
        match &request.input {
            DiscoveryInput::Page { url } => {
                let page = pages::fetch(&*self.transport, url)?;
                extract::from_text(
                    &page.text,
                    &extract::Origin::Page(page.url),
                    model.as_deref(),
                    &mut out,
                );
            }
            DiscoveryInput::Text {
                supplied_text_id,
                text,
            } => {
                extract::from_text(
                    text,
                    &extract::Origin::Pasted(supplied_text_id.clone()),
                    model.as_deref(),
                    &mut out,
                );
            }
            DiscoveryInput::Seeds => {
                let discogs = self.discogs()?;
                if discogs.is_none() && model.is_none() {
                    return Err(AdapterError::Invalid(
                        "Add a Discogs token or a model in Settings to discover from seeds.".into(),
                    ));
                }
                if request.seeds.is_empty() {
                    return Err(AdapterError::Invalid(
                        "Add discovery seeds in Settings, or rate some tracks, first.".into(),
                    ));
                }
                let seeds = rotate(&request.seeds);
                let mut discogs_error = None;
                if let Some(d) = &discogs {
                    if let Err(e) = expand(d, &seeds, request.limit, &mut out) {
                        if matches!(e, AdapterError::Auth(_)) {
                            return Err(e);
                        }
                        discogs_error = Some(e);
                    }
                }
                if let Some(m) = &model {
                    let wanted = request.limit.saturating_sub(out.len()).min(10);
                    if wanted > 0 {
                        // Model failures do not discard what Discogs found.
                        if let Err(e) =
                            suggest::suggest(m.as_ref(), discogs.as_ref(), &seeds, wanted, &mut out)
                        {
                            if out.is_empty() {
                                return Err(discogs_error.unwrap_or(e));
                            }
                        }
                    }
                }
                if let (Some(e), true) = (discogs_error, out.is_empty()) {
                    return Err(e);
                }
            }
        }
        out.proposals.truncate(request.limit.max(1));
        Ok(out.proposals)
    }
}

/// Starts from a different seed each hour so refreshes cover all seeds
/// within the request budget.
fn rotate(seeds: &[Seed]) -> Vec<Seed> {
    let hour = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 3600)
        .unwrap_or(0) as usize;
    let mut s = seeds.to_vec();
    if !s.is_empty() {
        let n = s.len();
        s.rotate_left(hour % n);
    }
    s
}

#[cfg(test)]
mod tests;
