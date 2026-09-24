use std::time::Duration;

use cd_core::adapters::{AdapterError, AdapterResult};
use serde_json::Value;
use url::Url;

/// Deliberately has no Debug implementation: headers and URLs may contain keys.
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    /// Sent as `application/x-www-form-urlencoded` instead of JSON.
    pub form: Option<Vec<(String, String)>>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET",
            url: url.into(),
            headers: vec![],
            body: None,
            form: None,
        }
    }
}

#[derive(Default)]
pub struct Response {
    pub status: u16,
    pub body: String,
    pub retry_after_ms: Option<u64>,
    /// Redirect target, returned rather than followed.
    pub location: Option<String>,
}

impl Response {
    pub fn checked(self) -> AdapterResult<Self> {
        match self.status {
            200..=299 => Ok(self),
            401 | 403 => Err(AdapterError::Auth(
                "Credentials were rejected. Update them in Settings and test the connection.".into(),
            )),
            429 => Err(AdapterError::RateLimited {
                retry_after_ms: self.retry_after_ms,
            }),
            500..=599 => Err(AdapterError::Unavailable(
                "The service is temporarily unavailable. Retry later.".into(),
            )),
            _ => Err(AdapterError::Invalid(format!(
                "The service returned HTTP {}. Check the endpoint and configuration.",
                self.status
            ))),
        }
    }
    pub fn json(self) -> AdapterResult<Value> {
        serde_json::from_str(&self.checked()?.body)
            .map_err(|_| AdapterError::Invalid("The service returned invalid JSON.".into()))
    }
}

pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> AdapterResult<Response>;
}

pub struct Http {
    agent: ureq::Agent,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(90)))
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into(),
        }
    }
}

/// Credentials may be sent over plain HTTP only to loopback model/slskd services.
pub fn endpoint(value: &str) -> AdapterResult<Url> {
    let url = Url::parse(value)
        .map_err(|_| AdapterError::Invalid("Enter a complete HTTP or HTTPS endpoint.".into()))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AdapterError::Invalid(
            "Use HTTPS, or HTTP on localhost. Put credentials in the credential fields, not the URL.".into(),
        ));
    }
    Ok(url)
}

impl Transport for Http {
    fn send(&self, request: Request) -> AdapterResult<Response> {
        let mut builder = ureq::http::Request::builder()
            .method(request.method)
            .uri(&request.url)
            .header(
                "User-Agent",
                "CrateDigger/0.1 (https://github.com/timini/crate-digger)",
            );
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let bytes = match (request.body, request.form) {
            (_, Some(form)) => {
                builder = builder.header("Content-Type", "application/x-www-form-urlencoded");
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(form)
                    .finish()
                    .into_bytes()
            }
            (Some(body), None) => {
                builder = builder.header("Content-Type", "application/json");
                serde_json::to_vec(&body)
                    .map_err(|_| AdapterError::Invalid("Cannot encode request.".into()))?
            }
            (None, None) => Vec::new(),
        };
        let request = builder
            .body(bytes)
            .map_err(|_| AdapterError::Invalid("Invalid request configuration.".into()))?;
        let mut response = self.agent.run(request).map_err(|_| {
            AdapterError::Unavailable(
                "Cannot reach the service. Check its endpoint and that it is running.".into(),
            )
        })?;
        let status = response.status().as_u16();
        let retry_after_ms = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| s.saturating_mul(1000).min(3_600_000));
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_string()
            .map_err(|_| AdapterError::Invalid("Response exceeded 2 MB or could not be read.".into()))?;
        Ok(Response {
            status,
            body,
            retry_after_ms,
            location,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_credentials_in_endpoints_and_remote_plaintext() {
        for url in [
            "https://user:password@example.com",
            "http://example.com",
            "https://example.com?key=secret",
            "file:///tmp/x",
        ] {
            assert!(endpoint(url).is_err());
        }
        assert!(endpoint("http://localhost:11434/v1").is_ok());
        assert!(endpoint("https://api.example.com/v1").is_ok());
    }
    #[test]
    fn service_errors_do_not_echo_secrets() {
        for status in [401, 403, 404, 429, 500] {
            let result = Response {
                status,
                body: "secret-token".into(),
                ..Default::default()
            }
            .checked();
            assert!(!result.err().unwrap().to_string().contains("secret-token"));
        }
    }
}
