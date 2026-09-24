use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};

use cd_core::adapters::{AdapterError, DiscoveryInput, DiscoveryRequest, DiscoverySource, Seed};
use cd_core::domain::SeedKind;
use serde_json::{json, Value};

use super::extract::{parse_lines, Origin};
use super::pages::{fetch, html_to_text, public_url, robots_allows};
use super::*;
use crate::credentials::MemoryStore;
use crate::llm::{LlmClient, StructuredRequest};
use crate::testing::FakeWeb;

fn discogs_url(path: &str, query: &[(&str, &str)]) -> String {
    let mut url = url::Url::parse(discogs::API).unwrap();
    url.set_path(path);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.to_string()
}

fn search(kind: &str, q: &str) -> String {
    discogs_url("/database/search", &[("type", kind), ("per_page", "5"), ("q", q)])
}

fn release_json(id: u64, title: &str, artist: &str, label: &str, tracks: &[&str]) -> String {
    json!({
        "id": id,
        "title": title,
        "year": 2020,
        "uri": format!("https://www.discogs.com/release/{id}-x"),
        "artists": [{"name": artist, "anv": "", "join": ""}],
        "labels": [{"name": label, "catno": format!("CAT{id}")}],
        "tracklist": tracks.iter().map(|t| json!({"position": "A1", "title": t, "type_": "track"})).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Seed artist "Seed Artist" (id 1) on "Fixture Label" (id 10), which also
/// releases "Other Artist".
fn discogs_web() -> FakeWeb {
    FakeWeb::default()
        .route(
            &search("artist", "Seed Artist"),
            200,
            json!({"results": [{"id": 2, "title": "Seed Artists", "type": "artist"}, {"id": 1, "title": "Seed Artist (2)", "type": "artist"}]}).to_string(),
        )
        .route(
            &discogs_url("/artists/1/releases", &[("sort", "year"), ("sort_order", "desc"), ("per_page", "10")]),
            200,
            json!({"releases": [
                {"id": 50, "type": "master", "main_release": 100, "role": "Main", "year": 2021},
                {"id": 101, "type": "release", "role": "Remix", "year": 2020}
            ]})
            .to_string(),
        )
        .route(
            &discogs_url("/releases/100", &[]),
            200,
            release_json(100, "Seed EP", "Seed Artist (2)", "Fixture Label", &["Deep Cut (Original Mix)", "Heading"]),
        )
        .route(
            &search("label", "Fixture Label"),
            200,
            json!({"results": [{"id": 10, "title": "Fixture Label", "type": "label"}]}).to_string(),
        )
        .route(
            &discogs_url("/labels/10/releases", &[("per_page", "30")]),
            200,
            json!({"releases": [
                {"id": 100, "artist": "Seed Artist", "year": 2021},
                {"id": 200, "artist": "Other Artist", "year": 2019}
            ]})
            .to_string(),
        )
        .route(
            &discogs_url("/releases/200", &[]),
            200,
            release_json(200, "Other EP", "Other Artist", "Fixture Label", &["Warehouse Tool", "Warehouse Tool (Dub)"]),
        )
}

fn seed(kind: SeedKind, value: &str) -> Seed {
    Seed {
        kind,
        value: value.into(),
    }
}

#[test]
fn names_and_mixes_are_cleaned() {
    assert_eq!(clean_artist("Seed Artist (2)"), "Seed Artist");
    assert_eq!(clean_artist("Name*"), "Name");
    assert_eq!(clean_artist("Band (UK)"), "Band (UK)");
    assert_eq!(
        split_mix("Track (Dub Mix)"),
        ("Track".into(), Some("Dub Mix".into()))
    );
    assert_eq!(
        split_mix("Track [Someone Remix]"),
        ("Track".into(), Some("Someone Remix".into()))
    );
    assert_eq!(split_mix("Track (Part 1)"), ("Track (Part 1)".into(), None));
    assert_eq!(split_mix("(Original Mix)"), ("(Original Mix)".into(), None));
}

#[test]
fn discogs_expands_artist_then_label_to_other_artists() {
    let web = Arc::new(discogs_web());
    let d = discogs::Discogs::new("tok".into(), web.clone(), DISCOGS_BUDGET);
    let mut out = Collector::default();
    expand(&d, &[seed(SeedKind::Artist, "Seed Artist")], 10, &mut out).unwrap();
    let got: Vec<(String, String, Option<String>, String)> = out
        .proposals
        .iter()
        .map(|p| {
            (
                p.artist.clone(),
                p.title.clone(),
                p.mix.clone(),
                p.reasons[0].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "Seed Artist".into(),
                "Deep Cut".into(),
                Some("Original Mix".into()),
                "Released by Seed Artist, one of your seeds".into()
            ),
            (
                "Seed Artist".into(),
                "Heading".into(),
                None,
                "Released by Seed Artist, one of your seeds".into()
            ),
            (
                "Other Artist".into(),
                "Warehouse Tool".into(),
                None,
                "On Fixture Label, which also releases Seed Artist".into()
            ),
            (
                "Other Artist".into(),
                "Warehouse Tool".into(),
                Some("Dub".into()),
                "On Fixture Label, which also releases Seed Artist".into()
            ),
        ]
    );
    let e = &out.proposals[2].evidence[0];
    assert_eq!(e.source_kind, "discogs");
    assert_eq!(
        e.source_url.as_deref(),
        Some("https://www.discogs.com/release/200-x")
    );
    assert!(
        e.excerpt.contains("Other EP, Fixture Label CAT200, 2020"),
        "{}",
        e.excerpt
    );
    // The token goes only in the Authorization header.
    for (url, headers) in web.seen.lock().unwrap().iter() {
        assert!(!url.contains("tok"));
        assert!(headers.contains(&("Authorization".into(), "Discogs token=tok".into())));
    }
    // Release 100 was read once even though the label lists it too.
    assert_eq!(
        web.urls().iter().filter(|u| u.ends_with("/releases/100")).count(),
        1
    );
}

#[test]
fn discogs_budget_limits_requests_and_keeps_partial_results() {
    let web = Arc::new(discogs_web());
    let d = discogs::Discogs::new("tok".into(), web.clone(), 3);
    let mut out = Collector::default();
    expand(&d, &[seed(SeedKind::Artist, "Seed Artist")], 10, &mut out).unwrap();
    assert_eq!(web.urls().len(), 3);
    assert_eq!(out.len(), 2);
}

#[test]
fn rejected_token_stops_the_run() {
    let web = Arc::new(FakeWeb::default().route(&search("artist", "Seed Artist"), 401, "{}"));
    let d = discogs::Discogs::new("bad".into(), web, DISCOGS_BUDGET);
    let e = expand(
        &d,
        &[seed(SeedKind::Artist, "Seed Artist")],
        10,
        &mut Collector::default(),
    );
    assert!(matches!(e, Err(AdapterError::Auth(_))));
}

#[test]
fn unknown_artist_is_not_replaced_by_a_near_match() {
    let web = Arc::new(FakeWeb::default().route(
        &search("artist", "Nobody"),
        200,
        json!({"results": [{"id": 9, "title": "Nobody Else", "type": "artist"}]}).to_string(),
    ));
    let d = discogs::Discogs::new("tok".into(), web.clone(), DISCOGS_BUDGET);
    let mut out = Collector::default();
    expand(&d, &[seed(SeedKind::Artist, "Nobody")], 10, &mut out).unwrap();
    assert!(out.is_empty());
    assert_eq!(web.urls().len(), 1);
}

#[test]
fn tracklist_lines_are_read() {
    let text = "\
My favourite set
01. Alpha Unit - First Light (Extended Mix) [Fixture Label]
[00:05:10] Beta Signal – Second Wind
#3 Gamma Ray - Third Eye (Someone Remix)
A1 Delta - Fourth
- Epsilon - Fifth
2 Unlimited - Twilight Zone
808 State - Pacific
U2 - One
ID - ID
Visit https://example.com/a - b
This is a sentence - with a dash but it goes on far too long to be a plausible artist name at all really
";
    let got: Vec<(String, String, Option<String>, Option<String>)> = parse_lines(text)
        .into_iter()
        .map(|l| (l.artist, l.title, l.mix, l.label))
        .collect();
    let s = |v: &str| v.to_string();
    assert_eq!(
        got,
        vec![
            (
                s("Alpha Unit"),
                s("First Light"),
                Some(s("Extended Mix")),
                Some(s("Fixture Label"))
            ),
            (s("Beta Signal"), s("Second Wind"), None, None),
            (s("Gamma Ray"), s("Third Eye"), Some(s("Someone Remix")), None),
            (s("Delta"), s("Fourth"), None, None),
            (s("Epsilon"), s("Fifth"), None, None),
            (s("2 Unlimited"), s("Twilight Zone"), None, None),
            (s("808 State"), s("Pacific"), None, None),
            (s("U2"), s("One"), None, None),
        ]
    );
}

#[test]
fn robots_rules() {
    let robots = "\
User-agent: *
Disallow: /private
Allow: /private/ok
Disallow: /*.pdf$

User-agent: BadBot
Disallow: /
";
    assert!(robots_allows(robots, "/tracklists/1"));
    assert!(!robots_allows(robots, "/private/x"));
    assert!(robots_allows(robots, "/private/ok/1"));
    assert!(!robots_allows(robots, "/files/a.pdf"));
    assert!(robots_allows(robots, "/files/a.pdf?x"));
    assert!(!robots_allows(
        "User-agent: CrateDigger\nDisallow: /\n\nUser-agent: *\nAllow: /",
        "/a"
    ));
    assert!(robots_allows("User-agent: *\nDisallow:\n", "/a"));
    assert!(robots_allows("", "/a"));
}

#[test]
fn only_public_pages_can_be_read() {
    for bad in [
        "file:///etc/passwd",
        "http://localhost:5030/api",
        "http://127.0.0.1/",
        "http://192.168.1.10/",
        "http://10.0.0.1/",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/",
        "http://router/",
        "http://nas.local/",
        "https://user:pw@example.com/",
    ] {
        assert!(public_url(bad).is_err(), "{bad}");
    }
    assert_eq!(
        public_url("https://example.com/list#top").unwrap().as_str(),
        "https://example.com/list"
    );
}

#[test]
fn html_becomes_text_lines() {
    let html = "<html><head><title>T</title><style>p{}</style></head><body>\
        <script>ignore('Fake - Track')</script><!-- Hidden - Track -->\
        <ol><li>Alpha &amp; Beta - Song&nbsp;One</li><li>Gamma &#8211; Two</li></ol>\
        <p>Delta<br>Echo</p></body></html>";
    let text = html_to_text(html);
    assert!(!text.contains("Fake") && !text.contains("Hidden") && !text.contains("p{}"));
    assert!(text.contains("Alpha & Beta - Song One\n"));
    assert!(text.contains("Gamma – Two\n"));
    assert!(text.contains("Delta\nEcho\n"));
}

#[test]
fn pages_respect_robots_and_redirect_checks() {
    let page = "<ul><li>Alpha Unit - First Light</li></ul>";
    let web = FakeWeb::default()
        .route(
            "https://blocked.example/robots.txt",
            200,
            "User-agent: *\nDisallow: /",
        )
        .route("https://open.example/robots.txt", 404, "")
        .route("https://open.example/list", 200, page)
        .route("https://down.example/robots.txt", 503, "");
    assert!(matches!(
        fetch(&web, "https://blocked.example/list"),
        Err(AdapterError::Invalid(_))
    ));
    assert!(!web.urls().contains(&"https://blocked.example/list".to_string()));
    assert!(matches!(
        fetch(&web, "https://down.example/list"),
        Err(AdapterError::Unavailable(_))
    ));
    let got = fetch(&web, "https://open.example/list").unwrap();
    assert_eq!(got.text, "Alpha Unit - First Light\n");

    struct Redirect;
    impl crate::http::Transport for Redirect {
        fn send(&self, r: crate::http::Request) -> cd_core::adapters::AdapterResult<crate::http::Response> {
            Ok(match r.url.as_str() {
                "https://moved.example/a" => crate::http::Response {
                    status: 301,
                    location: Some("http://127.0.0.1:5030/api/v0/session".into()),
                    ..Default::default()
                },
                _ => crate::http::Response {
                    status: 404,
                    ..Default::default()
                },
            })
        }
    }
    assert!(fetch(&Redirect, "https://moved.example/a").is_err());
}

/// Replays scripted model replies.
struct Scripted(Mutex<VecDeque<Value>>);

impl Scripted {
    fn new(replies: Vec<Value>) -> Self {
        Self(Mutex::new(replies.into()))
    }
}

impl LlmClient for Scripted {
    fn structured(&self, _: &StructuredRequest) -> cd_core::adapters::AdapterResult<Value> {
        Ok(self.0.lock().unwrap().pop_front().unwrap_or(json!({})))
    }
}

#[test]
fn model_extraction_keeps_only_tracks_found_in_the_text() {
    let text =
        "Last night the opener played the Alpha Unit track First Light, what a record.\nThen INJECTED.";
    let model = Scripted::new(vec![json!({"tracks": [
        {"artist": "Alpha Unit", "title": "First Light", "mix": null, "label": null},
        {"artist": "Invented Artist", "title": "Made Up", "mix": null, "label": null},
        {"artist": "INJECTED", "title": "Payload", "mix": null, "label": null}
    ]})]);
    let mut out = Collector::default();
    extract::from_text(text, &Origin::Pasted("t1".into()), Some(&model), &mut out);
    assert_eq!(out.len(), 1);
    let p = &out.proposals[0];
    assert_eq!(
        (p.artist.as_str(), p.title.as_str()),
        ("Alpha Unit", "First Light")
    );
    assert_eq!(p.evidence[0].supplied_text_id.as_deref(), Some("t1"));
    assert_eq!(p.evidence[0].source_kind, "pasted");
    assert!(p.evidence[0].excerpt.starts_with("Last night"));
}

#[test]
fn long_lines_give_a_short_excerpt() {
    let line = format!(
        "{} Kerri Chandler - Rain (Dub) {}",
        "a".repeat(300),
        "b".repeat(300)
    );
    let e = extract::excerpt_around(&line, "Rain");
    assert!(e.starts_with("... ") && e.ends_with(" ...") && e.contains("Kerri Chandler - Rain"));
    assert!(e.len() < 230, "{}", e.len());
    assert_eq!(extract::excerpt_around("short line", "missing"), "short line");
}

fn finish(suggestions: Value) -> Value {
    json!({"action": {"tool": "finish", "arguments": {"suggestions": suggestions}}})
}

#[test]
fn model_suggestions_are_verified_only_when_discogs_confirms_them() {
    let web = Arc::new(discogs_web());
    let d = discogs::Discogs::new("tok".into(), web, DISCOGS_BUDGET);
    let model = Scripted::new(vec![finish(json!([
        {"artist": "Other Artist", "title": "Warehouse Tool", "release_id": 200, "reason": "Same label"},
        {"artist": "Other Artist", "title": "Not On That Record", "release_id": 200, "reason": "Guess"},
        {"artist": "Nobody", "title": "Imagined", "release_id": null, "reason": "Vibes"}
    ]))]);
    let mut out = Collector::default();
    suggest::suggest(
        &model,
        Some(&d),
        &[seed(SeedKind::Artist, "Seed Artist")],
        Some("Warm-up: dubby and spacious"),
        5,
        &mut out,
    )
    .unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(out.proposals[0].evidence[0].source_kind, "discogs");
    assert!(out.proposals[0].reasons[0].contains("confirmed on Discogs"));
    for p in &out.proposals[1..] {
        assert!(p.evidence.is_empty(), "{p:?}");
        assert!(p.reasons.iter().any(|r| r.contains("No source confirms")));
    }
}

fn live(web: FakeWeb, store: MemoryStore, model: &str) -> LiveSource {
    LiveSource {
        name: LIVE_SOURCE,
        config: Arc::new(RwLock::new(Connections {
            llm_model: model.into(),
            ..Connections::default()
        })),
        secrets: Arc::new(store),
        transport: Arc::new(web),
    }
}

#[test]
fn live_source_needs_a_connection_for_seeds_but_not_for_pasted_text() {
    let source = live(FakeWeb::default(), MemoryStore::default(), "");
    let seeds = DiscoveryRequest {
        seeds: vec![seed(SeedKind::Artist, "Seed Artist")],
        limit: 10,
        input: DiscoveryInput::Seeds,
        brief: None,
    };
    assert!(matches!(source.discover(&seeds), Err(AdapterError::Invalid(_))));
    let pasted = DiscoveryRequest {
        seeds: vec![],
        limit: 10,
        input: DiscoveryInput::Text {
            supplied_text_id: "t".into(),
            text: "Alpha Unit - First Light".into(),
        },
        brief: None,
    };
    assert_eq!(source.discover(&pasted).unwrap().len(), 1);

    let store = MemoryStore::default();
    store
        .set(crate::credentials::Credential::Discogs, Some("tok"))
        .unwrap();
    let source = live(discogs_web(), store, "");
    let found = source.discover(&seeds).unwrap();
    assert_eq!(found.len(), 4);
    let empty = DiscoveryRequest {
        seeds: vec![],
        ..seeds
    };
    assert!(matches!(source.discover(&empty), Err(AdapterError::Invalid(_))));
}

#[test]
fn an_artists_own_label_is_described_plainly() {
    let web = Arc::new(
        FakeWeb::default()
            .route(
                &search("label", "Seed Artist"),
                200,
                json!({"results": [{"id": 11, "title": "Seed Artist", "type": "label"}]}).to_string(),
            )
            .route(
                &discogs_url("/labels/11/releases", &[("per_page", "30")]),
                200,
                json!({"releases": [{"id": 200, "artist": "Other Artist", "year": 2019}]}).to_string(),
            )
            .route(
                &discogs_url("/releases/200", &[]),
                200,
                release_json(
                    200,
                    "Other EP",
                    "Other Artist",
                    "Seed Artist",
                    &["Warehouse Tool"],
                ),
            ),
    );
    let d = discogs::Discogs::new("tok".into(), web, DISCOGS_BUDGET);
    let mut out = Collector::default();
    let mut seen = std::collections::HashSet::new();
    expand_label(&d, "Seed Artist", Some("Seed Artist"), 10, &mut seen, &mut out).unwrap();
    assert_eq!(out.proposals[0].reasons[0], "On Seed Artist's own label");
}
