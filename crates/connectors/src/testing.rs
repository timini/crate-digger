//! A fake web for tests: fixed responses by URL, 404 for anything else.
use std::collections::HashMap;
use std::sync::Mutex;

use cd_core::adapters::AdapterResult;

use crate::http::{Request, Response, Transport};

/// A request as sent: URL and headers.
pub type Sent = (String, Vec<(String, String)>);

#[derive(Default)]
pub struct FakeWeb {
    routes: HashMap<String, (u16, String)>,
    pub seen: Mutex<Vec<Sent>>,
}

impl FakeWeb {
    pub fn route(mut self, url: &str, status: u16, body: impl Into<String>) -> Self {
        self.routes.insert(url.to_string(), (status, body.into()));
        self
    }

    pub fn urls(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(u, _)| u.clone()).collect()
    }
}

impl Transport for FakeWeb {
    fn send(&self, request: Request) -> AdapterResult<Response> {
        self.seen
            .lock()
            .unwrap()
            .push((request.url.clone(), request.headers.clone()));
        let (status, body) = self
            .routes
            .get(&request.url)
            .cloned()
            .unwrap_or((404, String::new()));
        Ok(Response {
            status,
            body,
            retry_after_ms: None,
            location: None,
        })
    }
}
