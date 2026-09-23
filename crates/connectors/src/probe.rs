//! Connection tests shown in Settings. Each makes one cheap authenticated request.
use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult};

use crate::config::Connections;
use crate::credentials::{Credential, SecretStore};
use crate::http::{Request, Transport};

pub fn probe(
    service: &str,
    config: &Connections,
    secrets: &dyn SecretStore,
    transport: Arc<dyn Transport>,
) -> AdapterResult<String> {
    config.validate()?;
    if service == "llm" {
        // A model that cannot follow a reply schema is unusable, so test that directly.
        return crate::llm::check(&*crate::llm::client(config, secrets, transport)?);
    }
    let (mut req, credential, header, prefix) = match service {
        "discogs" => (
            Request::get("https://api.discogs.com/oauth/identity"),
            Credential::Discogs,
            "Authorization",
            "Discogs token=",
        ),
        "youtube" => (
            Request::get("https://www.googleapis.com/youtube/v3/videos?part=id&id=dQw4w9WgXcQ"),
            Credential::Youtube,
            "X-Goog-Api-Key",
            "",
        ),
        "slskd" => (
            Request::get(format!(
                "{}/api/v0/session",
                config.slskd_endpoint.trim_end_matches('/')
            )),
            Credential::Slskd,
            "X-API-Key",
            "",
        ),
        _ => return Err(AdapterError::Invalid("Unknown connection.".into())),
    };
    match secrets.get(credential)? {
        Some(secret) => req.headers.push((header.into(), format!("{prefix}{secret}"))),
        None => {
            return Err(AdapterError::Auth(
                "Save this service's credentials in Settings first.".into(),
            ))
        }
    }
    transport.send(req)?.json()?;
    Ok("Connection succeeded.".into())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::credentials::MemoryStore;
    use crate::http::Response;

    type Seen = (String, Vec<(String, String)>);

    struct Fake {
        status: u16,
        seen: Mutex<Vec<Seen>>,
    }

    impl Fake {
        fn new(status: u16) -> Arc<Self> {
            Arc::new(Self {
                status,
                seen: Mutex::new(vec![]),
            })
        }
    }

    impl Transport for Fake {
        fn send(&self, request: Request) -> AdapterResult<Response> {
            self.seen.lock().unwrap().push((request.url, request.headers));
            Ok(Response {
                status: self.status,
                body: "{}".into(),
                retry_after_ms: None,
                location: None,
            })
        }
    }

    #[test]
    fn missing_credentials_fail_before_any_request() {
        let fake = Fake::new(200);
        let store = MemoryStore::default();
        for service in ["discogs", "youtube", "slskd"] {
            let e = probe(service, &Connections::default(), &store, fake.clone()).unwrap_err();
            assert!(matches!(e, AdapterError::Auth(_)));
        }
        assert!(fake.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn each_service_gets_only_its_own_secret() {
        let store = MemoryStore::default();
        store.set(Credential::Discogs, Some("discogs-secret")).unwrap();
        store.set(Credential::Youtube, Some("youtube-secret")).unwrap();
        store
            .set(Credential::SoulseekPassword, Some("soulseek-secret"))
            .unwrap();
        let fake = Fake::new(200);
        probe("discogs", &Connections::default(), &store, fake.clone()).unwrap();
        probe("youtube", &Connections::default(), &store, fake.clone()).unwrap();
        let seen = fake.seen.lock().unwrap();
        let discogs = format!("{:?}", seen[0]);
        let youtube = format!("{:?}", seen[1]);
        assert!(discogs.contains("Discogs token=discogs-secret") && !discogs.contains("youtube-secret"));
        assert!(youtube.contains("youtube-secret") && !youtube.contains("discogs-secret"));
        assert!(!format!("{seen:?}").contains("soulseek-secret"));
    }

    #[test]
    fn rejected_credentials_are_not_echoed() {
        let store = MemoryStore::default();
        store.set(Credential::Discogs, Some("rejected-secret")).unwrap();
        let e = probe("discogs", &Connections::default(), &store, Fake::new(401)).unwrap_err();
        assert!(matches!(e, AdapterError::Auth(_)));
        assert!(!e.to_string().contains("rejected-secret"));
    }

    #[test]
    fn removing_a_credential_clears_it() {
        let store = MemoryStore::default();
        store.set(Credential::Youtube, Some("x")).unwrap();
        store.set(Credential::Youtube, None).unwrap();
        assert_eq!(store.get(Credential::Youtube).unwrap(), None);
    }
}
