use cd_core::adapters::{AdapterError, AdapterResult};
use serde::{Deserialize, Serialize};

/// Only non-secret configuration. Unknown fields are rejected at the IPC boundary.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Connections {
    pub llm_provider: String,
    pub llm_endpoint: String,
    pub llm_model: String,
    pub slskd_endpoint: String,
    pub external_slskd: bool,
    /// Where an external slskd saves finished downloads.
    pub slskd_downloads_dir: String,
    pub enabled: bool,
    /// The central catalogue service, once deployed.
    pub central_endpoint: String,
    /// The app's Google OAuth client id (desktop type) for catalogue sign-in.
    pub google_client_id: String,
}
impl Default for Connections {
    fn default() -> Self {
        Self {
            llm_provider: "openai_compatible".into(),
            llm_endpoint: "http://localhost:11434/v1".into(),
            llm_model: String::new(),
            slskd_endpoint: "http://127.0.0.1:5030".into(),
            external_slskd: false,
            slskd_downloads_dir: String::new(),
            enabled: false,
            central_endpoint: String::new(),
            google_client_id: String::new(),
        }
    }
}
impl Connections {
    pub fn validate(&self) -> AdapterResult<()> {
        crate::http::endpoint(&self.llm_endpoint)?;
        crate::http::endpoint(&self.slskd_endpoint)?;
        if !self.central_endpoint.is_empty() {
            crate::http::endpoint(&self.central_endpoint)?;
        }
        if self.google_client_id.len() > 200 {
            return Err(AdapterError::Invalid("The Google client id is too long.".into()));
        }
        if !matches!(self.llm_provider.as_str(), "openai_compatible" | "anthropic") {
            return Err(AdapterError::Invalid("Choose a supported model provider.".into()));
        }
        if self.llm_model.len() > 200 {
            return Err(AdapterError::Invalid("Model name is too long.".into()));
        }
        Ok(())
    }
}
