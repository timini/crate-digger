//! Signing in to the catalogue with Google, as a desktop app: the system
//! browser opens Google's page, Google redirects to a one-off listener on
//! 127.0.0.1, and the code is exchanged with PKCE. The refresh token lives
//! in the OS keychain; ID tokens only in memory.
//! https://developers.google.com/identity/protocols/oauth2/native-app

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cd_core::adapters::{AdapterError, AdapterResult};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::credentials::{Credential, SecretStore};
use crate::http::{Request, Transport};
use crate::metadata::base64url;

const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const REVOKE: &str = "https://oauth2.googleapis.com/revoke";

pub struct GoogleAuth {
    pub client_id: String,
    pub secrets: Arc<dyn SecretStore>,
    pub transport: Arc<dyn Transport>,
    /// (ID token, valid until)
    token: Mutex<Option<(String, Instant)>>,
}

/// A sign-in in progress: the page to open and the listener waiting for Google.
pub struct Pending {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
}

fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// The email in an ID token, for showing who is signed in. Not verified
/// here; the service verifies every token it receives.
pub fn email_of(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["email"].as_str().map(str::to_string)
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl GoogleAuth {
    pub fn new(client_id: String, secrets: Arc<dyn SecretStore>, transport: Arc<dyn Transport>) -> Self {
        Self {
            client_id,
            secrets,
            transport,
            token: Mutex::new(None),
        }
    }

    pub fn signed_in(&self) -> AdapterResult<bool> {
        Ok(self.secrets.get(Credential::GoogleRefresh)?.is_some())
    }

    /// Start a sign-in: returns the page to open in the browser.
    pub fn begin(&self) -> AdapterResult<Pending> {
        if self.client_id.trim().is_empty() {
            return Err(AdapterError::Invalid(
                "The catalogue is not set up yet: add its Google client id in Settings.".into(),
            ));
        }
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|_| AdapterError::Unavailable("Cannot open a local port for sign-in.".into()))?;
        let port = listener
            .local_addr()
            .map_err(|_| AdapterError::Unavailable("No local port.".into()))?
            .port();
        let redirect = format!("http://127.0.0.1:{port}");
        let verifier = random();
        let state = random();
        let mut url = Url::parse(AUTH).expect("static url");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid email")
            .append_pair("code_challenge", &base64url(&Sha256::digest(verifier.as_bytes())))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
        Ok(Pending {
            url: url.to_string(),
            listener,
            redirect,
            verifier,
            state,
        })
    }

    /// Wait for Google's redirect, then exchange the code. Returns the email.
    pub fn finish(&self, pending: Pending, timeout: Duration) -> AdapterResult<Option<String>> {
        let code = wait_for_code(&pending.listener, &pending.state, timeout)?;
        let mut form = vec![
            ("code".to_string(), code),
            ("client_id".to_string(), self.client_id.clone()),
            ("redirect_uri".to_string(), pending.redirect),
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code_verifier".to_string(), pending.verifier),
        ];
        if let Some(secret) = self.secrets.get(Credential::GoogleClientSecret)? {
            form.push(("client_secret".into(), secret));
        }
        let v = self.token_request(form)?;
        let refresh = v["refresh_token"]
            .as_str()
            .ok_or_else(|| AdapterError::Invalid("Google did not return a refresh token.".into()))?;
        self.secrets.set(Credential::GoogleRefresh, Some(refresh))?;
        let email = v["id_token"].as_str().and_then(email_of);
        self.remember(&v);
        Ok(email)
    }

    fn token_request(&self, form: Vec<(String, String)>) -> AdapterResult<Value> {
        let mut req = Request::get(TOKEN);
        req.method = "POST";
        req.form = Some(form);
        let res = self.transport.send(req)?;
        if res.status == 400 || res.status == 401 {
            return Err(AdapterError::Auth(
                "Google refused the sign-in. Sign in again in Settings.".into(),
            ));
        }
        res.json()
    }

    fn remember(&self, v: &Value) {
        if let Some(t) = v["id_token"].as_str() {
            let ttl = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(120);
            *self.token.lock().unwrap() = Some((t.to_string(), Instant::now() + Duration::from_secs(ttl)));
        }
    }

    /// A current ID token, refreshed when needed.
    pub fn id_token(&self) -> AdapterResult<String> {
        if let Some((t, until)) = self.token.lock().unwrap().as_ref() {
            if Instant::now() < *until {
                return Ok(t.clone());
            }
        }
        let refresh = self.secrets.get(Credential::GoogleRefresh)?.ok_or_else(|| {
            AdapterError::Auth("Sign in with Google in Settings to use the catalogue.".into())
        })?;
        let mut form = vec![
            ("refresh_token".to_string(), refresh),
            ("client_id".to_string(), self.client_id.clone()),
            ("grant_type".to_string(), "refresh_token".to_string()),
        ];
        if let Some(secret) = self.secrets.get(Credential::GoogleClientSecret)? {
            form.push(("client_secret".into(), secret));
        }
        let v = self.token_request(form)?;
        self.remember(&v);
        v["id_token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AdapterError::Invalid("Google returned no ID token.".into()))
    }

    /// Forget the sign-in here and ask Google to revoke it.
    pub fn sign_out(&self) -> AdapterResult<()> {
        if let Some(refresh) = self.secrets.get(Credential::GoogleRefresh)? {
            let mut req = Request::get(REVOKE);
            req.method = "POST";
            req.form = Some(vec![("token".into(), refresh)]);
            // Best effort: the local sign-in is removed either way.
            let _ = self.transport.send(req);
        }
        self.secrets.set(Credential::GoogleRefresh, None)?;
        *self.token.lock().unwrap() = None;
        Ok(())
    }
}

/// Accept Google's redirect on the loopback listener and read the code.
fn wait_for_code(listener: &TcpListener, state: &str, timeout: Duration) -> AdapterResult<String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| AdapterError::Unavailable("Sign-in listener failed.".into()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).ok();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).ok();
                let path = line.split_whitespace().nth(1).unwrap_or("/");
                let url = Url::parse(&format!("http://127.0.0.1{path}")).ok();
                let get = |k: &str| {
                    url.as_ref()
                        .and_then(|u| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.to_string()))
                };
                let (ok, message) = match (get("code"), get("state"), get("error")) {
                    (Some(code), Some(s), _) if s == state => {
                        (Some(code), "Signed in to Crate Digger. You can close this tab.")
                    }
                    (_, _, Some(_)) => (None, "Sign-in was cancelled. You can close this tab."),
                    // Browsers also ask for /favicon.ico; keep waiting for the real redirect.
                    _ => {
                        let _ = write!(
                            &stream,
                            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        continue;
                    }
                };
                let body = format!("<!doctype html><title>Crate Digger</title><p style=\"font-family:sans-serif\">{message}</p>");
                let _ = write!(
                    &stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                return ok.ok_or_else(|| AdapterError::Invalid("Sign-in was cancelled.".into()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(AdapterError::Invalid("Sign-in timed out. Try again.".into()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return Err(AdapterError::Unavailable("Sign-in listener failed.".into())),
        }
    }
}
