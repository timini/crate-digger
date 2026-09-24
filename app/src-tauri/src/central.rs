//! The app's connection to the central catalogue: Google sign-in, sharing
//! and backups. Configured in Settings once the service is deployed.

use std::sync::{Arc, RwLock};

use cd_connectors::central::google::GoogleAuth;
use cd_connectors::central::Central;
use cd_connectors::config::Connections;
use cd_connectors::credentials::SecretStore;
use cd_connectors::http::Http;
use cd_core::adapters::{AdapterError, AdapterResult, CentralLookup, CentralSync, SyncAck};

pub struct CentralService {
    connections: Arc<RwLock<Connections>>,
    secrets: Arc<dyn SecretStore>,
    /// Rebuilt when the endpoint or client id changes.
    current: RwLock<Option<(String, String, Arc<Central>)>>,
}

impl CentralService {
    pub fn new(connections: Arc<RwLock<Connections>>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            connections,
            secrets,
            current: RwLock::new(None),
        }
    }

    /// The configured client, or None if the catalogue is not set up.
    pub fn client(&self) -> Option<Arc<Central>> {
        let c = self.connections.read().unwrap().clone();
        if c.central_endpoint.trim().is_empty() || c.google_client_id.trim().is_empty() {
            return None;
        }
        if let Some((e, id, client)) = self.current.read().unwrap().as_ref() {
            if *e == c.central_endpoint && *id == c.google_client_id {
                return Some(client.clone());
            }
        }
        let transport = Arc::new(Http::default());
        let client = Arc::new(Central {
            endpoint: c.central_endpoint.clone(),
            auth: Arc::new(GoogleAuth::new(
                c.google_client_id.clone(),
                self.secrets.clone(),
                transport.clone(),
            )),
            transport,
        });
        *self.current.write().unwrap() = Some((c.central_endpoint, c.google_client_id, client.clone()));
        Some(client)
    }

    fn signed_in_client(&self) -> AdapterResult<Arc<Central>> {
        let client = self.client().ok_or_else(|| {
            AdapterError::Auth("The shared catalogue is not set up yet. Add it in Settings.".into())
        })?;
        if !client.auth.signed_in()? {
            return Err(AdapterError::Auth(
                "Sign in with Google in Settings to share.".into(),
            ));
        }
        Ok(client)
    }
}

impl CentralSync for CentralService {
    fn submit(&self, key: &str, kind: &str, payload: &serde_json::Value) -> AdapterResult<SyncAck> {
        self.signed_in_client()?.submit(key, kind, payload)
    }
}

/// Reading the catalogue is optional: when not signed in, lookups find nothing.
impl CentralLookup for CentralService {
    fn lookup(
        &self,
        keys: &[cd_protocol::RecordingKey],
    ) -> AdapterResult<Vec<Option<cd_protocol::CatalogueEntry>>> {
        match self.signed_in_client() {
            Ok(c) => c.lookup(keys),
            Err(_) => Ok(vec![]),
        }
    }

    fn features(
        &self,
        recording_id: &str,
        version: &cd_protocol::FeatureVersion,
    ) -> AdapterResult<cd_protocol::FeaturesResponse> {
        self.signed_in_client()?.features(recording_id, version)
    }
}
