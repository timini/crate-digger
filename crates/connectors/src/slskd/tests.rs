use std::sync::Mutex;

use serde_json::json;

use super::*;
use crate::http::Response;

type Route = Box<dyn Fn(&str, &str, &Option<Value>) -> Option<(u16, Value)> + Send + Sync>;

/// Answers by method and path; records every request.
/// Method, URL, headers and body of a request.
type Seen = (String, String, Vec<(String, String)>, Option<Value>);

struct FakeSlskd {
    routes: Vec<Route>,
    seen: Mutex<Vec<Seen>>,
}

impl FakeSlskd {
    fn new(routes: Vec<Route>) -> Arc<Self> {
        Arc::new(Self {
            routes,
            seen: Mutex::default(),
        })
    }
    fn calls(&self, method: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, ..)| m == method)
            .map(|(_, u, ..)| u.clone())
            .collect()
    }
}

impl Transport for FakeSlskd {
    fn send(&self, r: Request) -> AdapterResult<Response> {
        let path = Url::parse(&r.url).unwrap().path().to_string();
        self.seen.lock().unwrap().push((
            r.method.to_string(),
            r.url.clone(),
            r.headers.clone(),
            r.body.clone(),
        ));
        let (status, body) = self
            .routes
            .iter()
            .find_map(|f| f(r.method, &path, &r.body))
            .unwrap_or((404, json!({})));
        Ok(Response {
            status,
            body: body.to_string(),
            ..Default::default()
        })
    }
}

fn slskd(web: Arc<FakeSlskd>, downloads: &Path) -> Slskd {
    Slskd {
        endpoint: "http://127.0.0.1:5030".into(),
        api_key: "key-123".into(),
        transport: web,
        downloads_dir: downloads.to_path_buf(),
        search_timeout: Duration::from_millis(10),
        poll: Duration::from_millis(1),
    }
}

fn responses() -> Value {
    json!([
        {"username": "dj one", "hasFreeUploadSlot": true, "queueLength": 0, "uploadSpeed": 900000, "files": [
            {"filename": "@@a\\Music\\Alpha Unit - First Light.flac", "size": 30000000, "length": 300, "extension": "flac"},
            {"filename": "@@a\\Music\\cover.jpg", "size": 1000, "extension": "jpg"},
            {"filename": "@@a\\Music\\Alpha Unit - Locked.flac", "size": 1, "isLocked": true}
        ]},
        {"username": "dj two", "hasFreeUploadSlot": false, "queueLength": 12, "uploadSpeed": 100000, "files": [
            {"filename": "@@b\\Alpha Unit - First Light.mp3", "size": 12000000, "bitRate": 320, "length": 301, "extension": ""}
        ]}
    ])
}

fn search_routes(hits: Value) -> Vec<Route> {
    vec![
        Box::new(|m, p, _| (m == "POST" && p == "/api/v0/searches").then(|| (200, json!({})))),
        Box::new(move |m, p, _| (m == "GET" && p.ends_with("/responses")).then(|| (200, hits.clone()))),
        Box::new(|m, p, _| {
            (m == "GET" && p.starts_with("/api/v0/searches/")).then(|| (200, json!({"isComplete": true})))
        }),
        Box::new(|m, _, _| (m == "DELETE").then(|| (204, json!({})))),
    ]
}

#[test]
fn search_text_leaves_out_features_and_punctuation() {
    assert_eq!(
        search_text("Alpha Unit feat. Singer", "First Light (Pt. 2)", None),
        "alpha unit first light pt 2"
    );
    assert_eq!(
        search_text("Alpha Unit", "First Light", Some("Extended Mix")),
        "alpha unit first light extended"
    );
    assert_eq!(
        search_text("Alpha Unit", "First Light", Some("Original Mix")),
        "alpha unit first light"
    );
}

#[test]
fn search_returns_unlocked_audio_with_peer_details() {
    let web = FakeSlskd::new(search_routes(responses()));
    let dir = tempfile::tempdir().unwrap();
    let s = slskd(web.clone(), dir.path());
    let results = s
        .search(&AcquisitionQuery {
            artist: "Alpha Unit".into(),
            title: "First Light".into(),
            mix: None,
        })
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].result_id,
        "dj one\n@@a\\Music\\Alpha Unit - First Light.flac"
    );
    assert_eq!(results[0].duration_ms, Some(300_000));
    assert_eq!(results[0].free_slot, Some(true));
    assert_eq!(results[1].format.as_deref(), Some("mp3"));
    assert_eq!(results[1].bitrate_kbps, Some(320));
    // The key goes in a header, the search is deleted afterwards.
    let seen = web.seen.lock().unwrap();
    assert!(seen
        .iter()
        .all(|(_, u, h, _)| !u.contains("key-123") && h.contains(&("X-API-Key".into(), "key-123".into()))));
    let post = seen.iter().find(|(m, ..)| m == "POST").unwrap();
    assert_eq!(post.3.as_ref().unwrap()["searchText"], "alpha unit first light");
    drop(seen);
    assert_eq!(web.calls("DELETE").len(), 1);
}

#[test]
fn a_mix_search_with_no_results_falls_back_to_artist_and_title() {
    let web = FakeSlskd::new(search_routes(json!([])));
    let dir = tempfile::tempdir().unwrap();
    slskd(web.clone(), dir.path())
        .search(&AcquisitionQuery {
            artist: "Alpha Unit".into(),
            title: "First Light".into(),
            mix: Some("Dub Mix".into()),
        })
        .unwrap();
    let texts: Vec<String> = web
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, ..)| m == "POST")
        .map(|(_, _, _, b)| b.as_ref().unwrap()["searchText"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        texts,
        vec!["alpha unit first light dub", "alpha unit first light"]
    );
}

fn transfer(id: &str, state: &str) -> Value {
    json!({"id": id, "filename": "@@a\\Music\\Alpha Unit - First Light.flac", "size": 4, "state": state, "bytesTransferred": 2})
}

#[test]
fn enqueue_reuses_a_live_transfer_and_encodes_the_user_name() {
    let dir = tempfile::tempdir().unwrap();
    let result = SearchResult {
        result_id: "dj one\n@@a\\Music\\Alpha Unit - First Light.flac".into(),
        size_bytes: 4,
        ..Default::default()
    };
    let existing = FakeSlskd::new(vec![Box::new(|m, p, _| {
        (m == "GET" && p == "/api/v0/transfers/downloads/dj%20one").then(|| {
            (
                200,
                json!({"directories": [{"files": [transfer("t1", "Queued, Remotely")]}]}),
            )
        })
    })]);
    let id = slskd(existing.clone(), dir.path())
        .enqueue(&result, "k", dir.path())
        .unwrap();
    assert!(id.starts_with("dj one\nt1\n"));
    assert!(existing.calls("POST").is_empty());

    let fresh = FakeSlskd::new(vec![
        Box::new(|m, p, _| {
            (m == "GET" && p == "/api/v0/transfers/downloads/dj%20one").then(|| {
                (
                    200,
                    json!({"directories": [{"files": [transfer("old", "Completed, Errored")]}]}),
                )
            })
        }),
        Box::new(|m, p, body| {
            (m == "POST" && p == "/api/v0/transfers/downloads/dj%20one").then(|| {
                assert_eq!(body.as_ref().unwrap()[0]["size"], 4);
                (
                    201,
                    json!({"enqueued": [transfer("t2", "Requested")], "failed": []}),
                )
            })
        }),
    ]);
    let id = slskd(fresh, dir.path())
        .enqueue(&result, "k", dir.path())
        .unwrap();
    assert!(id.starts_with("dj one\nt2\n"));
}

#[test]
fn transfer_states_map_to_status_and_the_file_is_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Music")).unwrap();
    let file = dir.path().join("Music/Alpha Unit - First Light.flac");
    std::fs::write(&file, b"abcd").unwrap();
    let id = transfer_id("dj one", "t1", "@@a\\Music\\Alpha Unit - First Light.flac", 4);
    let status = |state: &'static str| {
        let web = FakeSlskd::new(vec![Box::new(move |m, p, _| {
            (m == "GET" && p == "/api/v0/transfers/downloads/dj%20one/t1")
                .then(|| (200, transfer("t1", state)))
        })]);
        slskd(web, dir.path()).status(&id).unwrap()
    };
    assert_eq!(status("Queued, Remotely"), TransferStatus::Queued);
    assert_eq!(
        status("InProgress"),
        TransferStatus::InProgress { bytes: 2, total: 4 }
    );
    assert_eq!(
        status("Completed, Succeeded"),
        TransferStatus::Completed { path: file.clone() }
    );
    assert_eq!(status("Completed, Cancelled"), TransferStatus::Cancelled);
    assert!(
        matches!(status("Completed, Rejected"), TransferStatus::Failed { reason } if reason.contains("Rejected"))
    );

    // The same name with a different size is not the download.
    std::fs::write(&file, b"abcdef").unwrap();
    assert!(matches!(
        status("Completed, Succeeded"),
        TransferStatus::Failed { .. }
    ));
    let gone = FakeSlskd::new(vec![]);
    assert!(matches!(
        slskd(gone, dir.path()).status(&id).unwrap(),
        TransferStatus::Failed { .. }
    ));
}

#[test]
fn rejected_api_key_is_an_auth_error() {
    let web = FakeSlskd::new(vec![Box::new(|_, _, _| Some((401, json!({}))))]);
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        slskd(web, dir.path()).logged_in(),
        Err(AdapterError::Auth(_))
    ));
}
