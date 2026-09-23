//! YouTube Data API search with oEmbed confirmation. Every stored link is a
//! video that YouTube confirmed exists at lookup time; nothing is guessed.
use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult, VideoLookup, VideoMatch, VideoQuery};
use cd_core::identity::normalize::{classify_mix, fold, parse_artists, title_key};
use serde_json::Value;
use url::Url;

use crate::credentials::{Credential, SecretStore};
use crate::http::{Request, Response, Transport};

const API: &str = "https://www.googleapis.com/youtube/v3";
/// Results below this are dropped rather than kept as alternatives.
pub const KEEP_MIN: f64 = 0.4;
const MAX_KEPT: usize = 4;

pub struct YouTube {
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Arc<dyn Transport>,
}

/// A video ID from any common YouTube link form, or None.
pub fn video_id(link: &str) -> Option<String> {
    let url = Url::parse(link.trim()).ok()?;
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .trim_start_matches("m.");
    let id = match host {
        "youtu.be" => url.path_segments()?.next()?.to_string(),
        "youtube.com" | "music.youtube.com" => {
            let mut segments = url.path_segments()?;
            match segments.next()? {
                "watch" => url.query_pairs().find(|(k, _)| k == "v")?.1.into_owned(),
                "shorts" | "embed" | "live" | "v" => segments.next()?.to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(id)
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// ISO 8601 durations as YouTube reports them, for example PT1H2M3S.
pub fn parse_duration(iso: &str) -> Option<i64> {
    let rest = iso.strip_prefix("PT").or_else(|| iso.strip_prefix('P'))?;
    let (mut total, mut number) = (0i64, String::new());
    for c in rest.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: i64 = number.parse().ok()?;
        number.clear();
        total += n * match c {
            'H' => 3_600_000,
            'M' => 60_000,
            'S' => 1_000,
            'D' => 86_400_000,
            'T' => 0,
            _ => return None,
        };
    }
    number.is_empty().then_some(total)
}

const OFF_TOPIC: &[&str] = &[
    "live",
    "cover",
    "reaction",
    "karaoke",
    "tutorial",
    "slowed",
    "reverb",
    "sped",
    "nightcore",
    "lesson",
    "review",
    "full album",
    "megamix",
];
const VERSION_WORDS: &[&str] = &[
    "remix",
    "dub",
    "edit",
    "vip",
    "bootleg",
    "rework",
    "refix",
    "instrumental",
    "acapella",
];

fn has_word(haystack: &str, word: &str) -> bool {
    format!(" {haystack} ").contains(&format!(" {word} "))
}

/// How likely a video is this recording, from 0 to 1. The title must match;
/// the artist, the mix, the channel and the duration adjust the score.
pub fn score(query: &VideoQuery, title: &str, channel: &str, duration_ms: Option<i64>) -> f64 {
    let video = fold(title);
    let channel = fold(channel);
    let channel_artist = channel.trim_end_matches(" topic").trim().to_string();
    let wanted_title = title_key(&query.title);
    if wanted_title.is_empty() || !video.contains(&wanted_title) {
        return 0.0;
    }
    let mut s: f64 = 0.45;
    let artists = parse_artists(&query.artist);
    let artist_named = artists
        .main
        .iter()
        .any(|a| !a.is_empty() && (video.contains(a.as_str()) || channel_artist.contains(a.as_str())));
    if artist_named {
        s += 0.25;
        // Auto-generated "Artist - Topic" channels carry the released audio.
        if channel.ends_with(" topic") {
            s += 0.15;
        }
    } else {
        s -= 0.2;
    }
    let query_text = fold(&format!(
        "{} {} {}",
        query.artist,
        query.title,
        query.mix.as_deref().unwrap_or("")
    ));
    let mix = classify_mix(query.mix.as_deref());
    if mix.is_specific() {
        let mix_words: Vec<String> = fold(query.mix.as_deref().unwrap_or(""))
            .split(' ')
            .filter(|w| !w.is_empty() && *w != "mix")
            .map(str::to_string)
            .collect();
        if mix_words.iter().all(|w| video.contains(w.as_str())) {
            s += 0.1;
        } else {
            s -= 0.3;
        }
    } else if VERSION_WORDS
        .iter()
        .any(|w| has_word(&video, w) && !has_word(&query_text, w))
    {
        // An original was asked for and this names another version.
        s -= 0.3;
    }
    if OFF_TOPIC
        .iter()
        .any(|w| has_word(&video, w) && !has_word(&query_text, w))
    {
        s -= 0.4;
    }
    match (query.duration_ms, duration_ms) {
        (_, Some(d)) if d > 20 * 60_000 => s -= 0.4,
        (Some(known), Some(d)) if (known - d).abs() <= 5_000 => s += 0.1,
        (Some(known), Some(d)) if (known - d).abs() > 30_000 => s -= 0.2,
        _ => {}
    }
    s.clamp(0.0, 1.0)
}

impl YouTube {
    fn key(&self) -> AdapterResult<String> {
        self.secrets
            .get(Credential::Youtube)?
            .ok_or_else(|| AdapterError::Auth("Add a YouTube Data API key in Settings.".into()))
    }

    fn api(&self, key: &str, path: &str, query: &[(&str, &str)]) -> AdapterResult<Value> {
        let mut url = Url::parse(API).expect("static url");
        url.path_segments_mut().expect("base url").push(path);
        url.query_pairs_mut().extend_pairs(query);
        let mut req = Request::get(url.to_string());
        req.headers.push(("X-Goog-Api-Key".into(), key.to_string()));
        let response = self.transport.send(req)?;
        // Quota exhaustion is a 403 but not a credential problem.
        if response.status == 403 && response.body.contains("quotaExceeded") {
            return Err(AdapterError::RateLimited {
                retry_after_ms: Some(3_600_000),
            });
        }
        response.json()
    }

    /// Confirms a video exists and can be linked, and reads its title and channel.
    pub fn confirm(&self, id: &str) -> AdapterResult<Option<(String, String)>> {
        let mut url = Url::parse("https://www.youtube.com/oembed").expect("static url");
        url.query_pairs_mut()
            .append_pair("format", "json")
            .append_pair("url", &watch_url(id));
        let Response { status, body, .. } = self.transport.send(Request::get(url.to_string()))?;
        match status {
            200 => {
                let v: Value = serde_json::from_str(&body)
                    .map_err(|_| AdapterError::Invalid("YouTube returned an unexpected response.".into()))?;
                Ok(Some((
                    v["title"].as_str().unwrap_or_default().to_string(),
                    v["author_name"].as_str().unwrap_or_default().to_string(),
                )))
            }
            // Private, removed or not linkable.
            400..=499 => Ok(None),
            _ => Err(AdapterError::Unavailable(
                "YouTube is not responding. Retry later.".into(),
            )),
        }
    }

    /// The user's own link: a real, reachable video, or an error saying why not.
    pub fn user_link(&self, link: &str) -> AdapterResult<VideoMatch> {
        let id = video_id(link)
            .ok_or_else(|| AdapterError::Invalid("That is not a YouTube video link.".into()))?;
        let (title, channel) = self
            .confirm(&id)?
            .ok_or_else(|| AdapterError::Invalid("YouTube says that video is private or removed.".into()))?;
        Ok(VideoMatch {
            url: watch_url(&id),
            video_id: id,
            title: Some(title).filter(|t| !t.is_empty()),
            channel: Some(channel).filter(|c| !c.is_empty()),
            duration_ms: None,
            confidence: 1.0,
        })
    }
}

impl VideoLookup for YouTube {
    fn id(&self) -> &str {
        "youtube"
    }

    fn lookup(&self, query: &VideoQuery) -> AdapterResult<Vec<VideoMatch>> {
        let key = self.key()?;
        let mut q = format!("{} {}", query.artist, query.title);
        if let Some(mix) = query
            .mix
            .as_deref()
            .filter(|_| classify_mix(query.mix.as_deref()).is_specific())
        {
            q.push(' ');
            q.push_str(mix);
        }
        let search = self.api(
            &key,
            "search",
            &[("part", "id"), ("type", "video"), ("maxResults", "8"), ("q", &q)],
        )?;
        let ids: Vec<String> = search["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["id"]["videoId"].as_str().map(str::to_string))
            .collect();
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let details = self.api(
            &key,
            "videos",
            &[("part", "snippet,contentDetails"), ("id", &ids.join(","))],
        )?;
        let mut scored: Vec<VideoMatch> = details["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let id = v["id"].as_str()?.to_string();
                let title = v["snippet"]["title"].as_str().unwrap_or_default().to_string();
                let channel = v["snippet"]["channelTitle"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let duration = v["contentDetails"]["duration"].as_str().and_then(parse_duration);
                let confidence = score(query, &title, &channel, duration);
                (confidence >= KEEP_MIN).then(|| VideoMatch {
                    url: watch_url(&id),
                    video_id: id,
                    title: Some(title),
                    channel: Some(channel),
                    duration_ms: duration,
                    confidence,
                })
            })
            .collect();
        scored.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        let mut confirmed = vec![];
        for m in scored.into_iter().take(MAX_KEPT) {
            if self.confirm(&m.video_id)?.is_some() {
                confirmed.push(m);
            }
        }
        Ok(confirmed)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::credentials::MemoryStore;
    use crate::testing::FakeWeb;

    fn q(artist: &str, title: &str, mix: Option<&str>, duration_ms: Option<i64>) -> VideoQuery {
        VideoQuery {
            artist: artist.into(),
            title: title.into(),
            mix: mix.map(str::to_string),
            duration_ms,
        }
    }

    #[test]
    fn links_and_durations_parse() {
        for link in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://m.youtube.com/shorts/dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(link).as_deref(), Some("dQw4w9WgXcQ"), "{link}");
        }
        for bad in [
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/short",
            "not a url",
        ] {
            assert_eq!(video_id(bad), None, "{bad}");
        }
        assert_eq!(parse_duration("PT4M13S"), Some(253_000));
        assert_eq!(parse_duration("PT1H2M"), Some(3_720_000));
        assert_eq!(parse_duration("P0D"), Some(0));
        assert_eq!(parse_duration("4:13"), None);
    }

    #[test]
    fn scoring_prefers_the_right_version() {
        let original = q("Alpha Unit", "First Light", None, Some(300_000));
        let topic = score(&original, "First Light", "Alpha Unit - Topic", Some(301_000));
        let upload = score(&original, "Alpha Unit - First Light", "Some Label", Some(300_000));
        let remix = score(
            &original,
            "Alpha Unit - First Light (Beta Remix)",
            "Some Label",
            None,
        );
        let live = score(&original, "Alpha Unit - First Light (Live at Club)", "Fan", None);
        let set = score(
            &original,
            "Alpha Unit - First Light and more",
            "Fan",
            Some(3_600_000),
        );
        let other = score(&original, "Alpha Unit - Second Wind", "Alpha Unit - Topic", None);
        assert!(topic >= 0.9, "{topic}");
        assert!(upload >= 0.7, "{upload}");
        assert!(
            remix < 0.7 && live < KEEP_MIN && set < KEEP_MIN,
            "{remix} {live} {set}"
        );
        assert_eq!(other, 0.0);

        let dub = q("Alpha Unit", "First Light", Some("Dub Mix"), None);
        assert!(score(&dub, "Alpha Unit - First Light (Dub)", "Label", None) >= 0.7);
        assert!(score(&dub, "Alpha Unit - First Light", "Label", None) < 0.7);
    }

    fn web() -> FakeWeb {
        let search = {
            let mut u = Url::parse(API).unwrap();
            u.path_segments_mut().unwrap().push("search");
            u.query_pairs_mut().extend_pairs([
                ("part", "id"),
                ("type", "video"),
                ("maxResults", "8"),
                ("q", "Alpha Unit First Light"),
            ]);
            u.to_string()
        };
        let videos = {
            let mut u = Url::parse(API).unwrap();
            u.path_segments_mut().unwrap().push("videos");
            u.query_pairs_mut().extend_pairs([
                ("part", "snippet,contentDetails"),
                ("id", "aaaaaaaaaaa,bbbbbbbbbbb,ccccccccccc"),
            ]);
            u.to_string()
        };
        let oembed = |id: &str| {
            let mut u = Url::parse("https://www.youtube.com/oembed").unwrap();
            u.query_pairs_mut()
                .append_pair("format", "json")
                .append_pair("url", &watch_url(id));
            u.to_string()
        };
        let video = |id: &str, title: &str, channel: &str| json!({"id": id, "snippet": {"title": title, "channelTitle": channel}, "contentDetails": {"duration": "PT5M"}});
        FakeWeb::default()
            .route(
                &search,
                200,
                json!({"items": [
                    {"id": {"videoId": "aaaaaaaaaaa"}}, {"id": {"videoId": "bbbbbbbbbbb"}}, {"id": {"videoId": "ccccccccccc"}}
                ]})
                .to_string(),
            )
            .route(
                &videos,
                200,
                json!({"items": [
                    video("aaaaaaaaaaa", "First Light", "Alpha Unit - Topic"),
                    video("bbbbbbbbbbb", "Alpha Unit - First Light", "Uploader"),
                    video("ccccccccccc", "Something else entirely", "Uploader")
                ]})
                .to_string(),
            )
            .route(&oembed("aaaaaaaaaaa"), 200, json!({"title": "First Light", "author_name": "Alpha Unit - Topic"}).to_string())
            // b was removed after the search index saw it.
            .route(&oembed("bbbbbbbbbbb"), 404, "Not Found")
    }

    #[test]
    fn lookup_keeps_only_scored_and_confirmed_videos() {
        let store = MemoryStore::default();
        let web = Arc::new(web());
        let yt = YouTube {
            secrets: Arc::new(store),
            transport: web.clone(),
        };
        let query = q("Alpha Unit", "First Light", None, None);
        assert!(matches!(yt.lookup(&query), Err(AdapterError::Auth(_))));
        yt.secrets.set(Credential::Youtube, Some("yt-key")).unwrap();
        let got = yt.lookup(&query).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].video_id, "aaaaaaaaaaa");
        assert_eq!(got[0].duration_ms, Some(300_000));
        // The key is sent as a header, never in a URL.
        assert!(web.urls().iter().all(|u| !u.contains("yt-key")));
    }

    #[test]
    fn quota_exhaustion_is_retried_not_treated_as_a_bad_key() {
        let store = MemoryStore::default();
        store.set(Credential::Youtube, Some("k")).unwrap();
        struct Quota;
        impl Transport for Quota {
            fn send(&self, _: Request) -> AdapterResult<Response> {
                Ok(Response {
                    status: 403,
                    body: r#"{"error":{"errors":[{"reason":"quotaExceeded"}]}}"#.into(),
                    ..Default::default()
                })
            }
        }
        let yt = YouTube {
            secrets: Arc::new(store),
            transport: Arc::new(Quota),
        };
        assert!(matches!(
            yt.lookup(&q("A", "B", None, None)),
            Err(AdapterError::RateLimited { .. })
        ));
    }

    #[test]
    fn user_links_must_exist() {
        let yt = YouTube {
            secrets: Arc::new(MemoryStore::default()),
            transport: Arc::new(web()),
        };
        let ok = yt.user_link("https://youtu.be/aaaaaaaaaaa").unwrap();
        assert_eq!(ok.channel.as_deref(), Some("Alpha Unit - Topic"));
        assert!(yt.user_link("https://youtu.be/bbbbbbbbbbb").is_err());
        assert!(yt.user_link("https://example.com/x").is_err());
    }
}
