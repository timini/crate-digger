//! Library metadata from the audio itself: AcoustID matches the Chromaprint
//! fingerprint to MusicBrainz recordings, MusicBrainz gives the release and
//! label, and Discogs adds the genre. Each service is called within its
//! published rate limit.
//! https://acoustid.org/webservice, https://musicbrainz.org/doc/MusicBrainz_API

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cd_core::adapters::{AdapterError, AdapterResult, Identified, MetadataLookup, MetadataQuery};
use serde_json::Value;
use url::Url;

use crate::credentials::{Credential, SecretStore};
use crate::discovery::split_mix;
use crate::http::{Request, Transport};

const ACOUSTID: &str = "https://api.acoustid.org/v2/lookup";
const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2";

/// Keeps calls to each service at least a fixed interval apart.
#[derive(Default)]
pub struct Limiter {
    last: Mutex<HashMap<&'static str, Instant>>,
}

impl Limiter {
    pub fn wait(&self, service: &'static str, interval: Duration) {
        let mut last = self.last.lock().unwrap();
        if let Some(t) = last.get(service) {
            let since = t.elapsed();
            if since < interval {
                std::thread::sleep(interval - since);
            }
        }
        last.insert(service, Instant::now());
    }
}

/// Chromaprint's base64: URL-safe alphabet, no padding.
pub fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// The compressed, encoded fingerprint AcoustID expects.
pub fn encode_fingerprint(values: &[u32]) -> String {
    let config = rusty_chromaprint::Configuration::preset_test2();
    base64url(&rusty_chromaprint::FingerprintCompressor::from(&config).compress(values))
}

pub struct MetadataService {
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Arc<dyn Transport>,
    pub limiter: Arc<Limiter>,
    /// Test hook: 0 in tests, the services' published limits otherwise.
    pub pace: bool,
}

fn credit(artists: &Value) -> Option<String> {
    let list = artists.as_array()?;
    let mut s = String::new();
    for a in list {
        s.push_str(a["name"].as_str()?);
        s.push_str(a["joinphrase"].as_str().unwrap_or(""));
    }
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

impl MetadataService {
    fn get(&self, service: &'static str, interval_ms: u64, url: Url) -> AdapterResult<Value> {
        if self.pace {
            self.limiter.wait(service, Duration::from_millis(interval_ms));
        }
        let mut req = Request::get(url.to_string());
        req.headers.push(("Accept".into(), "application/json".into()));
        self.transport.send(req)?.json()
    }

    fn acoustid(&self, key: &str, q: &MetadataQuery) -> AdapterResult<Value> {
        let mut url = Url::parse(ACOUSTID).expect("static url");
        url.query_pairs_mut()
            .append_pair("client", key)
            .append_pair("format", "json")
            .append_pair("meta", "recordings releases")
            .append_pair("duration", &((q.duration_ms + 500) / 1000).to_string())
            .append_pair("fingerprint", &encode_fingerprint(&q.fingerprint));
        let v = self.get("acoustid", 340, url)?;
        if v["status"] != "ok" {
            let message = v["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(
                if message.contains("invalid API key") || message.contains("client") {
                    AdapterError::Auth("AcoustID rejected the application key. Check it in Settings.".into())
                } else {
                    AdapterError::Invalid(format!(
                        "AcoustID: {}",
                        message.chars().take(120).collect::<String>()
                    ))
                },
            );
        }
        Ok(v)
    }

    /// Release, year and label for a recording, from its earliest official release.
    fn musicbrainz(&self, recording: &str) -> AdapterResult<Vec<(String, String)>> {
        let mut url = Url::parse(&format!("{MUSICBRAINZ}/recording/{recording}")).expect("static url");
        url.query_pairs_mut()
            .append_pair("inc", "releases isrcs")
            .append_pair("fmt", "json");
        let rec = self.get("musicbrainz", 1_100, url)?;
        let mut fields = vec![];
        let mut releases: Vec<&Value> = rec["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["status"].as_str().is_none_or(|s| s == "Official"))
            .collect();
        releases.sort_by_key(|r| r["date"].as_str().unwrap_or("9999").to_string());
        if let Some(r) = releases.first() {
            if let Some(t) = r["title"].as_str() {
                fields.push(("release".into(), t.to_string()));
            }
            if let Some(y) = r["date"]
                .as_str()
                .and_then(|d| d.get(..4))
                .filter(|y| y.chars().all(|c| c.is_ascii_digit()))
            {
                fields.push(("year".into(), y.to_string()));
            }
            // MusicBrainz's release country, e.g. "GB" or "XW" for worldwide.
            if let Some(c) = r["country"].as_str().filter(|c| !c.is_empty()) {
                fields.push(("release_country".into(), c.to_string()));
            }
            if let Some(id) = r["id"].as_str() {
                let mut url = Url::parse(&format!("{MUSICBRAINZ}/release/{id}")).expect("static url");
                url.query_pairs_mut()
                    .append_pair("inc", "labels")
                    .append_pair("fmt", "json");
                let rel = self.get("musicbrainz", 1_100, url)?;
                if let Some(label) = rel["label-info"][0]["label"]["name"].as_str() {
                    fields.push(("label".into(), label.to_string()));
                }
            }
        }
        for isrc in rec["isrcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            fields.push(("isrc".into(), isrc.to_string()));
        }
        Ok(fields)
    }

    /// Genre (Discogs style, else genre) and label, when a token is saved.
    fn discogs(&self, artist: &str, title: &str) -> Vec<(String, String)> {
        let Ok(Some(token)) = self.secrets.get(Credential::Discogs) else {
            return vec![];
        };
        if self.pace {
            self.limiter.wait("discogs", Duration::from_millis(1_100));
        }
        let d = crate::discovery::discogs::Discogs::new(token, self.transport.clone(), 1);
        let Ok(hits) = d.search_raw("release", &[("artist", artist), ("track", title)]) else {
            return vec![];
        };
        let Some(hit) = hits.first() else { return vec![] };
        let mut out = vec![];
        let genre = hit["style"][0].as_str().or(hit["genre"][0].as_str());
        if let Some(g) = genre {
            out.push(("genre".into(), g.to_string()));
        }
        if let Some(l) = hit["label"][0].as_str() {
            out.push(("label".into(), crate::discovery::clean_artist(l)));
        }
        if let Some(c) = hit["country"].as_str().filter(|c| !c.is_empty()) {
            out.push(("release_country".into(), c.to_string()));
        }
        out
    }
}

impl MetadataLookup for MetadataService {
    fn lookup(&self, q: &MetadataQuery) -> AdapterResult<Vec<Identified>> {
        let key = self
            .secrets
            .get(Credential::Acoustid)?
            .ok_or_else(|| AdapterError::Auth("Add an AcoustID application key in Settings.".into()))?;
        let v = self.acoustid(&key, q)?;
        let mut out: Vec<Identified> = vec![];
        for result in v["results"].as_array().into_iter().flatten() {
            let score = result["score"].as_f64().unwrap_or(0.0);
            for rec in result["recordings"].as_array().into_iter().flatten() {
                let (Some(id), Some(title), Some(artist)) =
                    (rec["id"].as_str(), rec["title"].as_str(), credit(&rec["artists"]))
                else {
                    continue;
                };
                // The same recording can come back under several AcoustIDs.
                if out.iter().any(|o| o.external_ids.iter().any(|(_, v)| v == id)) {
                    continue;
                }
                let (title, mix) = split_mix(title);
                let mut fields = vec![("artist".into(), artist), ("title".into(), title)];
                fields.extend(mix.map(|m| ("mix".to_string(), m)));
                out.push(Identified {
                    score,
                    duration_ms: rec["duration"].as_f64().map(|s| (s * 1000.0) as i64),
                    fields,
                    discogs_fields: vec![],
                    external_ids: vec![
                        ("musicbrainz_recording".into(), id.to_string()),
                        (
                            "acoustid".into(),
                            result["id"].as_str().unwrap_or_default().to_string(),
                        ),
                    ],
                });
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(5);
        // Details only for the best match, to stay within MusicBrainz's limit.
        if let Some(best) = out.first_mut() {
            let recording = best.external_ids[0].1.clone();
            for (field, value) in self.musicbrainz(&recording)? {
                if field == "isrc" {
                    best.external_ids.push(("isrc".into(), value));
                } else {
                    best.fields.push((field, value));
                }
            }
            let get = |f: &str| best.fields.iter().find(|(k, _)| k == f).map(|(_, v)| v.clone());
            if let (Some(artist), Some(title)) = (get("artist"), get("title")) {
                best.discogs_fields = self.discogs(&artist, &title);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::credentials::MemoryStore;
    use crate::http::Response;

    #[test]
    fn base64url_matches_chromaprint() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        // Chromaprint header: algorithm 1 (test2), then the item count (00 00 03).
        let fp = encode_fingerprint(&[1, 2, 3]);
        assert!(fp.starts_with("AQAAA"), "{fp}");
    }

    #[test]
    fn limiter_spaces_calls() {
        let l = Limiter::default();
        let start = Instant::now();
        for _ in 0..3 {
            l.wait("x", Duration::from_millis(30));
        }
        assert!(start.elapsed() >= Duration::from_millis(60));
    }

    type Route = Box<dyn Fn(&Url) -> Option<Value> + Send + Sync>;
    struct Fake(Vec<Route>, Mutex<Vec<String>>);
    impl Transport for Fake {
        fn send(&self, r: Request) -> AdapterResult<Response> {
            let url = Url::parse(&r.url).unwrap();
            self.1.lock().unwrap().push(r.url.clone());
            let body = self.0.iter().find_map(|f| f(&url));
            Ok(Response {
                status: if body.is_some() { 200 } else { 404 },
                body: body.unwrap_or(json!({})).to_string(),
                ..Default::default()
            })
        }
    }

    fn service(routes: Vec<Route>, with_discogs: bool) -> (MetadataService, Arc<Fake>) {
        let store = MemoryStore::default();
        store.set(Credential::Acoustid, Some("app-key")).unwrap();
        if with_discogs {
            store.set(Credential::Discogs, Some("tok")).unwrap();
        }
        let web = Arc::new(Fake(routes, Mutex::default()));
        (
            MetadataService {
                secrets: Arc::new(store),
                transport: web.clone(),
                limiter: Arc::default(),
                pace: false,
            },
            web,
        )
    }

    fn query() -> MetadataQuery {
        MetadataQuery {
            fingerprint: vec![1, 2, 3],
            duration_ms: 392_400,
            artist: None,
            title: Some("track 01".into()),
        }
    }

    #[test]
    fn identifies_and_fills_release_label_and_genre() {
        let routes: Vec<Route> = vec![
            Box::new(|u| {
                (u.host_str() == Some("api.acoustid.org")).then(|| {
                    json!({"status": "ok", "results": [
                        {"id": "aid-1", "score": 0.96, "recordings": [
                            {"id": "rec-1", "title": "Rain (Dub)", "duration": 392.0, "artists": [
                                {"name": "Kerri Chandler", "joinphrase": " feat. "}, {"name": "Singer"}]}
                        ]},
                        {"id": "aid-2", "score": 0.4, "recordings": [
                            {"id": "rec-2", "title": "Other", "duration": 200.0, "artists": [{"name": "Someone"}]}
                        ]}
                    ]})
                })
            }),
            Box::new(|u| {
                u.path().ends_with("/recording/rec-1").then(|| {
                    json!({"isrcs": ["USXX1"], "releases": [
                        {"id": "rel-late", "title": "Remix Comp", "date": "2010", "status": "Official"},
                        {"id": "rel-1", "title": "Rain EP", "date": "1995-03", "status": "Official", "country": "US"}
                    ]})
                })
            }),
            Box::new(|u| {
                u.path()
                    .ends_with("/release/rel-1")
                    .then(|| json!({"label-info": [{"label": {"name": "Madhouse Records"}}]}))
            }),
            Box::new(|u| {
                (u.host_str() == Some("api.discogs.com"))
                    .then(|| json!({"results": [{"style": ["Deep House"], "genre": ["Electronic"], "label": ["Madhouse Records, Inc."]}]}))
            }),
        ];
        let (s, web) = service(routes, true);
        let out = s.lookup(&query()).unwrap();
        assert_eq!(out.len(), 2);
        let best = &out[0];
        let get = |f: &str| best.fields.iter().find(|(k, _)| k == f).map(|(_, v)| v.as_str());
        assert_eq!(get("artist"), Some("Kerri Chandler feat. Singer"));
        assert_eq!((get("title"), get("mix")), (Some("Rain"), Some("Dub")));
        assert_eq!(
            (get("release"), get("year"), get("label")),
            (Some("Rain EP"), Some("1995"), Some("Madhouse Records"))
        );
        assert_eq!(get("release_country"), Some("US"));
        assert_eq!(best.duration_ms, Some(392_000));
        assert!(best.external_ids.contains(&("isrc".into(), "USXX1".into())));
        assert_eq!(best.discogs_fields[0], ("genre".into(), "Deep House".into()));
        let first = &web.1.lock().unwrap()[0];
        assert!(
            first.contains("client=app-key")
                && first.contains("duration=392")
                && first.contains("fingerprint=AQAAA")
        );
    }

    #[test]
    fn missing_or_rejected_keys_are_auth_errors() {
        let (s, _) = service(vec![], false);
        s.secrets.set(Credential::Acoustid, None).unwrap();
        assert!(matches!(s.lookup(&query()), Err(AdapterError::Auth(_))));
        let routes: Vec<Route> = vec![Box::new(|_| {
            Some(json!({"status": "error", "error": {"code": 4, "message": "invalid API key"}}))
        })];
        let (s, _) = service(routes, false);
        assert!(matches!(s.lookup(&query()), Err(AdapterError::Auth(_))));
        let routes: Vec<Route> = vec![Box::new(|_| Some(json!({"status": "ok", "results": []})))];
        let (s, _) = service(routes, false);
        assert!(s.lookup(&query()).unwrap().is_empty());
    }
}
