//! Model suggestions from seeds. With a Discogs token the model can look
//! releases up, and each suggestion that names a release is checked by app
//! code against that release's tracklist. Anything unconfirmed is kept as an
//! unverified suggestion with no evidence.
use cd_core::adapters::{AdapterError, AdapterResult, CandidateProposal, Seed};
use cd_core::identity::normalize::{parse_artists, title_key};
use serde_json::{json, Value};

use super::discogs::Discogs;
use super::{split_mix, Collector};
use crate::llm::agent::{run, Limits, Tool, Toolbox};
use crate::llm::schema::Schema;
use crate::llm::{untrusted, LlmClient, Message};

struct DiscogsTools<'a> {
    discogs: Option<&'a Discogs>,
    tools: Vec<Tool>,
}

impl<'a> DiscogsTools<'a> {
    fn new(discogs: Option<&'a Discogs>) -> Self {
        let tools = if discogs.is_some() {
            vec![
                Tool {
                    name: "discogs_search",
                    description: "Search Discogs. kind is artist, label or release. Returns ids and titles.",
                    arguments: Schema::new(json!({
                        "type": "object",
                        "properties": {
                            "kind": {"type": "string", "enum": ["artist", "label", "release"]},
                            "query": {"type": "string", "minLength": 1, "maxLength": 120}
                        },
                        "required": ["kind", "query"],
                        "additionalProperties": false
                    }))
                    .expect("static schema"),
                },
                Tool {
                    name: "discogs_release",
                    description: "Read one Discogs release: title, label, year and tracklist.",
                    arguments: Schema::new(json!({
                        "type": "object",
                        "properties": {"id": {"type": "integer", "minimum": 1}},
                        "required": ["id"],
                        "additionalProperties": false
                    }))
                    .expect("static schema"),
                },
            ]
        } else {
            vec![]
        };
        Self { discogs, tools }
    }
}

impl Toolbox for DiscogsTools<'_> {
    fn tools(&self) -> &[Tool] {
        &self.tools
    }

    fn call(&self, name: &str, arguments: &Value) -> AdapterResult<Value> {
        let discogs = self
            .discogs
            .ok_or_else(|| AdapterError::Invalid("Discogs is not connected.".into()))?;
        match name {
            "discogs_search" => {
                let kind = arguments["kind"].as_str().unwrap_or("release");
                let query = arguments["query"].as_str().unwrap_or_default();
                let hits = discogs.search(kind, &[("q", query)])?;
                Ok(json!(hits
                    .iter()
                    .map(|h| json!({"id": h.id, "kind": h.kind, "title": h.title}))
                    .collect::<Vec<_>>()))
            }
            "discogs_release" => {
                let id = arguments["id"].as_u64().unwrap_or(0);
                let r = discogs.release(id)?;
                Ok(json!({
                    "id": r.id,
                    "title": r.title,
                    "year": r.year,
                    "label": r.label().map(|l| l.name.clone()),
                    "tracks": r.tracks().iter().map(|t| json!({
                        "artist": t.artist, "title": t.title, "mix": t.mix
                    })).collect::<Vec<_>>(),
                }))
            }
            _ => Err(AdapterError::Invalid("Unknown tool.".into())),
        }
    }
}

fn result_schema(limit: usize) -> Schema {
    Schema::new(json!({
        "type": "object",
        "properties": {"suggestions": {
            "type": "array",
            "maxItems": limit.max(1),
            "items": {
                "type": "object",
                "properties": {
                    "artist": {"type": "string", "minLength": 1, "maxLength": 120},
                    "title": {"type": "string", "minLength": 1, "maxLength": 120},
                    "release_id": {"type": ["integer", "null"], "minimum": 1},
                    "reason": {"type": "string", "minLength": 1, "maxLength": 200}
                },
                "required": ["artist", "title", "release_id", "reason"],
                "additionalProperties": false
            }
        }},
        "required": ["suggestions"],
        "additionalProperties": false
    }))
    .expect("static schema")
}

pub fn suggest(
    model: &dyn LlmClient,
    discogs: Option<&Discogs>,
    seeds: &[Seed],
    brief: Option<&str>,
    limit: usize,
    out: &mut Collector,
) -> AdapterResult<()> {
    let toolbox = DiscogsTools::new(discogs);
    let seeds_text = seeds
        .iter()
        .take(20)
        .map(|s| format!("{}: {}", s.kind, s.value))
        .collect::<Vec<_>>()
        .join("\n");
    let how = if discogs.is_some() {
        "Use the Discogs tools to find real releases and give the release id for each suggestion."
    } else {
        "Give release_id as null."
    };
    let fit = if brief.is_some() {
        " and that fit the playlist brief"
    } else {
        ""
    };
    let mut task = vec![
        Message::user(format!(
            "Suggest up to {limit} tracks, not already in the seeds, that a DJ who likes these seeds would want to hear{fit}. {how} Explain each choice in one sentence."
        )),
        untrusted("the user's discovery seeds", &seeds_text),
    ];
    if let Some(b) = brief {
        task.push(untrusted("the playlist brief", b));
    }
    let outcome = run(
        model,
        &toolbox,
        "You help a DJ discover music related to their seeds.",
        task,
        &result_schema(limit),
        Limits {
            max_steps: 6,
            ..Limits::default()
        },
    )?;
    for s in outcome.result["suggestions"].as_array().into_iter().flatten() {
        let (Some(artist), Some(title)) = (s["artist"].as_str(), s["title"].as_str()) else {
            continue;
        };
        let (title, mix) = split_mix(title);
        let model_reason = s["reason"].as_str().unwrap_or_default();
        let confirmed = match (discogs, s["release_id"].as_u64()) {
            (Some(d), Some(id)) => confirm(d, id, artist, &title)?,
            _ => None,
        };
        out.add(confirmed.unwrap_or_else(|| CandidateProposal {
            artist: artist.trim().to_string(),
            title: title.clone(),
            mix,
            label: None,
            release: None,
            reasons: vec![
                format!("Suggested by the model: {model_reason}"),
                "No source confirms this suggestion yet".into(),
            ],
            evidence: vec![],
        }));
    }
    Ok(())
}

/// App-side check: the release exists and lists this artist and title.
fn confirm(
    discogs: &Discogs,
    id: u64,
    artist: &str,
    title: &str,
) -> AdapterResult<Option<CandidateProposal>> {
    let release = match discogs.release(id) {
        Ok(r) => r,
        Err(AdapterError::Auth(m)) => return Err(AdapterError::Auth(m)),
        Err(_) => return Ok(None),
    };
    let wanted_artist = parse_artists(artist);
    let wanted_title = title_key(title);
    Ok(release
        .tracks()
        .iter()
        .find(|t| title_key(&t.title) == wanted_title && parse_artists(&t.artist).overlaps(&wanted_artist))
        .map(|t| {
            release.proposal(
                t,
                format!(
                    "Suggested by the model and confirmed on Discogs ({})",
                    release.title
                ),
            )
        }))
}
