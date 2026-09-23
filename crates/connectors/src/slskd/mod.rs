//! Soulseek through slskd (https://github.com/slskd/slskd), which runs as a
//! separate process: either one the app manages or one the user already
//! runs. slskd is AGPL-3.0; the app talks to it only over its HTTP API.
pub mod install;
pub mod process;
pub mod refine;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cd_core::adapters::{
    Acquirer, AcquisitionQuery, AdapterError, AdapterResult, SearchResult, TransferStatus,
};
use cd_core::identity::normalize::{classify_mix, fold, parse_artists, title_key};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::http::{Request, Transport};

pub const CONNECTOR: &str = "slskd";

const AUDIO: &[&str] = &[
    "mp3", "flac", "wav", "aiff", "aif", "m4a", "ogg", "opus", "alac", "ape", "wv",
];
const MAX_RESULTS: usize = 300;

/// One slskd instance's API.
#[derive(Clone)]
pub struct Slskd {
    pub endpoint: String,
    pub api_key: String,
    pub transport: Arc<dyn Transport>,
    /// Where finished downloads land, to find the file after a transfer.
    pub downloads_dir: PathBuf,
    pub search_timeout: Duration,
    pub poll: Duration,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerFile {
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    bit_rate: Option<u32>,
    #[serde(default)]
    length: Option<u64>,
    #[serde(default)]
    extension: Option<String>,
    #[serde(default)]
    is_locked: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerResponse {
    username: String,
    #[serde(default)]
    files: Vec<PeerFile>,
    #[serde(default)]
    has_free_upload_slot: bool,
    #[serde(default)]
    queue_length: u64,
    #[serde(default)]
    upload_speed: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transfer {
    id: String,
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    bytes_transferred: u64,
    #[serde(default)]
    exception: Option<String>,
}

/// Our transfer id: the peer, slskd's id, the remote file and its size.
fn transfer_id(username: &str, id: &str, filename: &str, size: u64) -> String {
    format!("{username}\n{id}\n{filename}\n{size}")
}

fn parse_transfer_id(t: &str) -> AdapterResult<(&str, &str, &str, u64)> {
    let mut parts = t.splitn(4, '\n');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(u), Some(i), Some(f), Some(s)) => Ok((u, i, f, s.parse().unwrap_or(0))),
        _ => Err(AdapterError::Invalid("Unknown transfer.".into())),
    }
}

fn result_parts(result_id: &str) -> AdapterResult<(&str, &str)> {
    result_id
        .split_once('\n')
        .ok_or_else(|| AdapterError::Invalid("Unknown search result.".into()))
}

/// Words for a Soulseek search: every word must match, so leave out
/// featured artists and punctuation.
pub fn search_text(artist: &str, title: &str, mix: Option<&str>) -> String {
    let main = parse_artists(artist).main.into_iter().next().unwrap_or_default();
    let mut words = format!("{main} {}", title_key(title));
    if let Some(mix) = mix.filter(|m| classify_mix(Some(m)).is_specific()) {
        words.push(' ');
        words.push_str(&fold(mix).replace(" mix", ""));
    }
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn remote_name(filename: &str) -> &str {
    filename.rsplit(['\\', '/']).next().unwrap_or(filename)
}

/// slskd places a finished file under its downloads folder in a
/// subfolder it chooses, and may rename it. Find it by name and size.
pub fn locate(dir: &Path, filename: &str, size: u64) -> Option<PathBuf> {
    let wanted = remote_name(filename);
    let wanted_fold = fold(wanted);
    let mut stack = vec![dir.to_path_buf()];
    let mut by_size = None;
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).ok()?.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.len() == size {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == wanted {
                    return Some(path);
                }
                if fold(&name) == wanted_fold {
                    by_size = Some(path);
                }
            }
        }
    }
    by_size
}

impl Slskd {
    fn url(&self, segments: &[&str]) -> AdapterResult<Url> {
        let mut url = crate::http::endpoint(&self.endpoint)?;
        url.path_segments_mut()
            .map_err(|_| AdapterError::Invalid("Invalid slskd endpoint.".into()))?
            .pop_if_empty()
            .extend(["api", "v0"])
            .extend(segments);
        Ok(url)
    }

    fn call(
        &self,
        method: &'static str,
        url: Url,
        body: Option<Value>,
    ) -> AdapterResult<crate::http::Response> {
        let mut req = Request::get(url.to_string());
        req.method = method;
        req.body = body;
        req.headers.push(("X-API-Key".into(), self.api_key.clone()));
        self.transport.send(req)
    }

    fn get(&self, segments: &[&str]) -> AdapterResult<Value> {
        self.call("GET", self.url(segments)?, None)?.json()
    }

    /// Whether slskd is signed in to the Soulseek network.
    pub fn logged_in(&self) -> AdapterResult<bool> {
        let server = self.get(&["server"])?;
        Ok(server["isLoggedIn"].as_bool().unwrap_or(false))
    }

    /// One search, waited for, as candidate results.
    pub fn search_text(&self, text: &str) -> AdapterResult<Vec<SearchResult>> {
        let id = uuid::Uuid::new_v4().to_string();
        self.call(
            "POST",
            self.url(&["searches"])?,
            Some(json!({
                "id": id,
                "searchText": text,
                "searchTimeout": self.search_timeout.as_millis() as u64,
                "filterResponses": true,
                "minimumResponseFileCount": 1,
                "responseLimit": 50,
                "fileLimit": 2000
            })),
        )?
        .checked()?;
        let deadline = Instant::now() + self.search_timeout + Duration::from_secs(10);
        loop {
            let state = self.get(&["searches", &id])?;
            if state["isComplete"].as_bool().unwrap_or(false) || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(self.poll);
        }
        let responses: Vec<PeerResponse> =
            serde_json::from_value(self.get(&["searches", &id, "responses"])?)
                .map_err(|_| AdapterError::Invalid("slskd returned unexpected search results.".into()))?;
        // Searches are kept by slskd until deleted.
        let _ = self.call("DELETE", self.url(&["searches", &id])?, None);
        let mut out = vec![];
        for r in responses {
            for f in r.files.into_iter().filter(|f| !f.is_locked) {
                let ext = f
                    .extension
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| {
                        remote_name(&f.filename)
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .to_ascii_lowercase();
                if !AUDIO.contains(&ext.as_str()) || f.filename.contains('\n') {
                    continue;
                }
                out.push(SearchResult {
                    result_id: format!("{}\n{}", r.username, f.filename),
                    filename: f.filename,
                    size_bytes: f.size,
                    duration_ms: f.length.map(|s| s * 1000),
                    format: Some(ext),
                    bitrate_kbps: f.bit_rate,
                    username: Some(r.username.clone()),
                    free_slot: Some(r.has_free_upload_slot),
                    queue_length: Some(r.queue_length),
                    upload_speed: Some(r.upload_speed),
                });
                if out.len() >= MAX_RESULTS {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    fn user_downloads(&self, username: &str) -> AdapterResult<Vec<Transfer>> {
        let response = self.call("GET", self.url(&["transfers", "downloads", username])?, None)?;
        if response.status == 404 {
            return Ok(vec![]);
        }
        let v = response.json()?;
        Ok(v["directories"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|d| d["files"].as_array().cloned().unwrap_or_default())
            .filter_map(|f| serde_json::from_value(f).ok())
            .collect())
    }
}

impl Acquirer for Slskd {
    fn id(&self) -> &str {
        CONNECTOR
    }

    fn search(&self, query: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>> {
        let specific = search_text(&query.artist, &query.title, query.mix.as_deref());
        let mut results = self.search_text(&specific)?;
        let plain = search_text(&query.artist, &query.title, None);
        if results.is_empty() && plain != specific {
            results = self.search_text(&plain)?;
        }
        Ok(results)
    }

    fn enqueue(&self, result: &SearchResult, _idempotency_key: &str, _dest: &Path) -> AdapterResult<String> {
        let (username, filename) = result_parts(&result.result_id)?;
        // A transfer for this file that is queued, running or done is reused.
        if let Some(t) = self.user_downloads(username)?.into_iter().find(|t| {
            t.filename == filename && (!t.state.contains("Completed") || t.state.contains("Succeeded"))
        }) {
            return Ok(transfer_id(username, &t.id, filename, t.size));
        }
        let v = self
            .call(
                "POST",
                self.url(&["transfers", "downloads", username])?,
                Some(json!([{ "filename": filename, "size": result.size_bytes }])),
            )?
            .json()?;
        let id = v["enqueued"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["filename"] == filename))
            .and_then(|t| t["id"].as_str())
            .ok_or_else(|| {
                AdapterError::Unavailable(
                    "The sharer did not accept the download request. They may be offline.".into(),
                )
            })?;
        Ok(transfer_id(username, id, filename, result.size_bytes))
    }

    fn status(&self, transfer: &str) -> AdapterResult<TransferStatus> {
        let (username, id, filename, size) = parse_transfer_id(transfer)?;
        let response = self.call("GET", self.url(&["transfers", "downloads", username, id])?, None)?;
        if response.status == 404 {
            return Ok(TransferStatus::Failed {
                reason: "slskd no longer has this transfer.".into(),
            });
        }
        let t: Transfer = serde_json::from_value(response.json()?)
            .map_err(|_| AdapterError::Invalid("slskd returned an unexpected transfer.".into()))?;
        let state = t.state.as_str();
        Ok(if state.contains("Succeeded") {
            match locate(&self.downloads_dir, filename, size) {
                Some(path) => TransferStatus::Completed { path },
                None => TransferStatus::Failed {
                    reason: "slskd finished the download but the file is not in its downloads folder.".into(),
                },
            }
        } else if state.contains("Cancelled") {
            TransferStatus::Cancelled
        } else if state.contains("Completed") {
            TransferStatus::Failed {
                reason: match t.exception.filter(|e| !e.is_empty()) {
                    Some(e) => format!("{state}: {}", e.chars().take(200).collect::<String>()),
                    None => state.to_string(),
                },
            }
        } else if state.contains("InProgress") || state.contains("Initializing") {
            TransferStatus::InProgress {
                bytes: t.bytes_transferred,
                total: t.size,
            }
        } else {
            TransferStatus::Queued
        })
    }

    fn cancel(&self, transfer: &str) -> AdapterResult<()> {
        let (username, id, _, _) = parse_transfer_id(transfer)?;
        let mut url = self.url(&["transfers", "downloads", username, id])?;
        url.query_pairs_mut().append_pair("remove", "true");
        self.call("DELETE", url, None)?.checked()?;
        Ok(())
    }
}

/// Finds the slskd to use at the moment of each call: the managed process
/// may restart on another port, and settings may switch to an external one.
pub trait Locate: Send + Sync {
    fn locate(&self) -> AdapterResult<Slskd>;
}

/// The acquirer the app registers. Searches that find no acceptable copy
/// can be widened by a model through `refine`.
pub struct SoulseekAcquirer {
    pub locate: Arc<dyn Locate>,
    pub model: Arc<dyn Fn() -> Option<Box<dyn crate::llm::LlmClient>> + Send + Sync>,
}

impl Acquirer for SoulseekAcquirer {
    fn id(&self) -> &str {
        CONNECTOR
    }

    fn search(&self, query: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>> {
        let slskd = self.locate.locate()?;
        let mut results = slskd.search(query)?;
        let usable = cd_core::acquisition::matching::assess(query, &results)
            .ranked
            .iter()
            .any(|a| a.acceptable);
        if !usable {
            if let Some(model) = (self.model)() {
                // The model only proposes search words; the matching rule still decides.
                if let Ok(more) = refine::refine(model.as_ref(), &slskd, query) {
                    results.extend(more);
                }
            }
        }
        Ok(results)
    }

    fn enqueue(&self, result: &SearchResult, key: &str, dest: &Path) -> AdapterResult<String> {
        self.locate.locate()?.enqueue(result, key, dest)
    }

    fn status(&self, transfer: &str) -> AdapterResult<TransferStatus> {
        self.locate.locate()?.status(transfer)
    }

    fn cancel(&self, transfer: &str) -> AdapterResult<()> {
        self.locate.locate()?.cancel(transfer)
    }
}

#[cfg(test)]
mod tests;
