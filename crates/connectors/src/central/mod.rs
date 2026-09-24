//! The central catalogue service (github.com/timini/crate-digger-service):
//! sharing, feature reuse and private backups, signed in with Google.

pub mod google;

use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult, CentralLookup, CentralSync, SyncAck};
use cd_protocol::backup::{BackupInfo, Snapshot};
use cd_protocol::*;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::http::{Request, Response, Transport};
use google::GoogleAuth;

pub struct Central {
    pub endpoint: String,
    pub auth: Arc<GoogleAuth>,
    pub transport: Arc<dyn Transport>,
}

impl Central {
    fn send(&self, method: &'static str, path: &str, body: Option<Value>) -> AdapterResult<Response> {
        let base = crate::http::endpoint(&self.endpoint)?;
        let mut req = Request::get(format!("{}{path}", base.as_str().trim_end_matches('/')));
        req.method = method;
        req.body = body;
        req.headers.push((
            "Authorization".into(),
            format!("Bearer {}", self.auth.id_token()?),
        ));
        let res = self.transport.send(req)?;
        if res.status == 401 {
            return Err(AdapterError::Auth(
                "The catalogue refused the sign-in. Sign in again in Settings.".into(),
            ));
        }
        Ok(res)
    }

    fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
    ) -> AdapterResult<T> {
        let res = self.send(method, path, body)?;
        if res.status == 400 || res.status == 404 || res.status == 413 {
            let p: Option<Problem> = serde_json::from_str(&res.body).ok();
            return Err(AdapterError::Invalid(
                p.map(|p| p.message)
                    .unwrap_or_else(|| format!("HTTP {}", res.status)),
            ));
        }
        serde_json::from_value(res.json()?)
            .map_err(|_| AdapterError::Invalid("The catalogue returned an unexpected response.".into()))
    }

    pub fn put_backup(&self, id: &str, snapshot: &Snapshot) -> AdapterResult<BackupInfo> {
        self.call(
            "PUT",
            &format!("/v1/backups/{id}"),
            Some(serde_json::to_value(snapshot).expect("serialisable")),
        )
    }

    pub fn list_backups(&self) -> AdapterResult<Vec<BackupInfo>> {
        self.call("GET", "/v1/backups", None)
    }

    pub fn get_backup(&self, id: &str) -> AdapterResult<Snapshot> {
        self.call("GET", &format!("/v1/backups/{id}"), None)
    }

    pub fn delete_backup(&self, id: &str) -> AdapterResult<()> {
        let res = self.send("DELETE", &format!("/v1/backups/{id}"), None)?;
        match res.status {
            204 | 200 | 404 => Ok(()),
            _ => res.checked().map(|_| ()),
        }
    }
}

impl CentralSync for Central {
    fn submit(&self, idempotency_key: &str, _kind: &str, payload: &Value) -> AdapterResult<SyncAck> {
        let contribution: Contribution = serde_json::from_value(payload.clone())
            .map_err(|_| AdapterError::Invalid("not a contribution".into()))?;
        let res: SubmitResponse = self.call(
            "POST",
            "/v1/contributions",
            Some(
                serde_json::to_value(SubmitRequest {
                    contributions: vec![contribution],
                })
                .expect("serialisable"),
            ),
        )?;
        match res.acks.into_iter().next() {
            Some(Ack::Accepted { .. }) => Ok(SyncAck {
                idempotency_key: idempotency_key.into(),
                duplicate: false,
            }),
            Some(Ack::Duplicate { .. }) => Ok(SyncAck {
                idempotency_key: idempotency_key.into(),
                duplicate: true,
            }),
            Some(Ack::Rejected { reason, .. }) if reason == "daily_limit" => Err(AdapterError::RateLimited {
                retry_after_ms: Some(3_600_000),
            }),
            Some(Ack::Rejected { reason, .. }) => Err(AdapterError::Invalid(reason)),
            None => Err(AdapterError::Invalid("no acknowledgement".into())),
        }
    }
}

impl CentralLookup for Central {
    fn lookup(&self, keys: &[RecordingKey]) -> AdapterResult<Vec<Option<CatalogueEntry>>> {
        let res: LookupResponse = self.call(
            "POST",
            "/v1/lookup",
            Some(
                serde_json::to_value(LookupRequest {
                    recordings: keys.to_vec(),
                })
                .expect("serialisable"),
            ),
        )?;
        Ok(res.results)
    }

    fn features(&self, recording_id: &str, version: &FeatureVersion) -> AdapterResult<FeaturesResponse> {
        self.call(
            "POST",
            "/v1/features",
            Some(
                serde_json::to_value(FeaturesRequest {
                    recording_id: recording_id.into(),
                    version: version.clone(),
                })
                .expect("serialisable"),
            ),
        )
    }
}

#[cfg(test)]
mod tests;
