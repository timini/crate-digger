use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;

use super::google::{email_of, GoogleAuth};
use super::*;
use crate::credentials::{Credential, MemoryStore, SecretStore};

type Route = Box<dyn Fn(&Request) -> Option<(u16, Value)> + Send + Sync>;

type Pairs = Vec<(String, String)>;
/// (method, url, form, headers)
type Seen = (String, String, Option<Pairs>, Pairs);

struct Fake {
    routes: Vec<Route>,
    seen: Mutex<Vec<Seen>>,
}

impl Transport for Fake {
    fn send(&self, r: Request) -> AdapterResult<Response> {
        let hit = self.routes.iter().find_map(|f| f(&r));
        self.seen.lock().unwrap().push((
            r.method.to_string(),
            r.url.clone(),
            r.form.clone(),
            r.headers.clone(),
        ));
        let (status, body) = hit.unwrap_or((404, json!({})));
        Ok(Response {
            status,
            body: body.to_string(),
            ..Default::default()
        })
    }
}

fn fake(routes: Vec<Route>) -> Arc<Fake> {
    Arc::new(Fake {
        routes,
        seen: Mutex::default(),
    })
}

fn id_token(email: &str) -> String {
    let payload = crate::metadata::base64url(json!({ "email": email, "sub": "123" }).to_string().as_bytes());
    format!("eyJhbGciOiJSUzI1NiJ9.{payload}.sig")
}

fn form_value(form: &Option<Vec<(String, String)>>, key: &str) -> Option<String> {
    form.as_ref()?
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn token_route() -> Route {
    Box::new(|r| {
        (r.url == "https://oauth2.googleapis.com/token").then(|| {
            let grant = form_value(&r.form, "grant_type").unwrap_or_default();
            (200, json!({ "id_token": id_token("dj@example.com"), "refresh_token": "refresh-1", "expires_in": 3600, "grant": grant }))
        })
    })
}

#[test]
fn email_is_read_from_an_id_token() {
    assert_eq!(
        email_of(&id_token("dj@example.com")).as_deref(),
        Some("dj@example.com")
    );
    assert_eq!(email_of("garbage"), None);
}

#[test]
fn sign_in_uses_pkce_on_a_loopback_redirect_and_keeps_the_refresh_token() {
    let web = fake(vec![token_route()]);
    let store = Arc::new(MemoryStore::default());
    let auth = GoogleAuth::new("client-1".into(), store.clone(), web.clone());
    assert!(!auth.signed_in().unwrap());
    let pending = auth.begin().unwrap();
    let url = url::Url::parse(&pending.url).unwrap();
    let q = |k: &str| {
        url.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.to_string())
            .unwrap()
    };
    assert_eq!(q("code_challenge_method"), "S256");
    assert!(q("redirect_uri").starts_with("http://127.0.0.1:"));
    let (redirect, state) = (q("redirect_uri"), q("state"));

    // The browser: a stray favicon request, then Google's redirect.
    let browser = std::thread::spawn(move || {
        let addr = redirect.trim_start_matches("http://");
        for path in ["/favicon.ico".to_string(), format!("/?code=abc&state={state}")] {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            if path.contains("code") {
                assert!(out.contains("Signed in to Crate Digger"));
            }
        }
    });
    let email = auth.finish(pending, Duration::from_secs(5)).unwrap();
    browser.join().unwrap();
    assert_eq!(email.as_deref(), Some("dj@example.com"));
    assert_eq!(
        store.get(Credential::GoogleRefresh).unwrap().as_deref(),
        Some("refresh-1")
    );
    let seen = web.seen.lock().unwrap();
    let exchange = &seen[0].2;
    assert_eq!(form_value(exchange, "code").as_deref(), Some("abc"));
    assert!(form_value(exchange, "code_verifier").is_some_and(|v| v.len() >= 43));
    drop(seen);

    // The token is cached; a fresh instance refreshes with the stored token.
    auth.id_token().unwrap();
    assert_eq!(web.seen.lock().unwrap().len(), 1);
    let later = GoogleAuth::new("client-1".into(), store.clone(), web.clone());
    later.id_token().unwrap();
    assert_eq!(
        form_value(&web.seen.lock().unwrap()[1].2, "grant_type").as_deref(),
        Some("refresh_token")
    );

    later.sign_out().unwrap();
    assert!(!later.signed_in().unwrap());
    assert!(matches!(later.id_token(), Err(AdapterError::Auth(_))));
}

#[test]
fn a_redirect_with_the_wrong_state_is_ignored() {
    let auth = GoogleAuth::new(
        "client-1".into(),
        Arc::new(MemoryStore::default()),
        fake(vec![token_route()]),
    );
    let pending = auth.begin().unwrap();
    let redirect = url::Url::parse(&pending.url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .to_string();
    std::thread::spawn(move || {
        let mut s = std::net::TcpStream::connect(redirect.trim_start_matches("http://")).unwrap();
        write!(s, "GET /?code=evil&state=forged HTTP/1.1\r\n\r\n").unwrap();
    });
    assert!(auth.finish(pending, Duration::from_millis(600)).is_err());
    assert!(!auth.signed_in().unwrap());
}

fn central(routes: Vec<Route>) -> (Central, Arc<Fake>) {
    let store = Arc::new(MemoryStore::default());
    store.set(Credential::GoogleRefresh, Some("refresh-1")).unwrap();
    let mut all = vec![token_route()];
    all.extend(routes);
    let web = fake(all);
    (
        Central {
            endpoint: "https://catalogue.example".into(),
            auth: Arc::new(GoogleAuth::new("client-1".into(), store, web.clone())),
            transport: web.clone(),
        },
        web,
    )
}

fn contribution() -> Value {
    json!({
        "idempotency_key": "k1",
        "recording": { "fingerprint_hash": null, "external_ids": [{ "source": "musicbrainz_recording", "id": "rec-1" }] },
        "metadata": { "artist": "A", "title": "T", "mix": null, "label": null, "release": null, "year": null, "duration_ms": null },
        "features": null, "references": [], "correction": false
    })
}

#[test]
fn acknowledgements_map_to_outbox_outcomes() {
    for (ack, expect) in [
        (json!({"status": "accepted", "idempotency_key": "k1"}), "ok"),
        (json!({"status": "duplicate", "idempotency_key": "k1"}), "dup"),
        (
            json!({"status": "rejected", "idempotency_key": "k1", "reason": "wrong_dimensions"}),
            "invalid",
        ),
        (
            json!({"status": "rejected", "idempotency_key": "k1", "reason": "daily_limit"}),
            "later",
        ),
    ] {
        let (c, web) = central(vec![Box::new(move |r| {
            r.url
                .ends_with("/v1/contributions")
                .then(|| (200, json!({ "acks": [ack.clone()] })))
        })]);
        let got = match c.submit("k1", "contribution", &contribution()) {
            Ok(a) if a.duplicate => "dup",
            Ok(_) => "ok",
            Err(AdapterError::Invalid(_)) => "invalid",
            Err(AdapterError::RateLimited { .. }) => "later",
            Err(e) => panic!("{e}"),
        };
        assert_eq!(got, expect);
        let seen = web.seen.lock().unwrap();
        let call = seen.iter().find(|s| s.1.ends_with("/v1/contributions")).unwrap();
        assert!(call
            .3
            .iter()
            .any(|(k, v)| k == "Authorization" && v.starts_with("Bearer ey")));
    }
    let (c, _) = central(vec![Box::new(|r| {
        r.url.contains("/v1/").then(|| (401, json!({"code": "sign_in"})))
    })]);
    assert!(matches!(
        c.submit("k1", "contribution", &contribution()),
        Err(AdapterError::Auth(_))
    ));
    let (c, _) = central(vec![Box::new(|r| {
        r.url.contains("/v1/").then(|| (503, json!({})))
    })]);
    assert!(matches!(
        c.submit("k1", "contribution", &contribution()),
        Err(AdapterError::Unavailable(_))
    ));
}

#[test]
fn backups_go_to_the_backup_routes() {
    let (c, web) = central(vec![
        Box::new(|r| {
            (r.method == "PUT" && r.url.ends_with("/v1/backups/b1")).then(|| {
                (
                    200,
                    json!({"id": "b1", "created_at_ms": 1, "size_bytes": 10, "version": 1}),
                )
            })
        }),
        Box::new(|r| {
            (r.method == "GET" && r.url.ends_with("/v1/backups")).then(|| {
                (
                    200,
                    json!([{"id": "b1", "created_at_ms": 1, "size_bytes": 10, "version": 1}]),
                )
            })
        }),
        Box::new(|r| (r.method == "DELETE").then(|| (204, json!(null)))),
    ]);
    let snap = cd_protocol::backup::Snapshot {
        version: 1,
        created_at_ms: 1,
        tracks: vec![],
        ratings: vec![],
        seeds: vec![],
        playlists: vec![],
    };
    assert_eq!(c.put_backup("b1", &snap).unwrap().id, "b1");
    assert_eq!(c.list_backups().unwrap().len(), 1);
    c.delete_backup("b1").unwrap();
    assert!(web.seen.lock().unwrap().iter().any(|s| s.0 == "DELETE"));
}
