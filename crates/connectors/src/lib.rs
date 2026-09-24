//! Network and credential boundaries. The domain crate stays offline.
pub mod config;
pub mod credentials;
pub mod discovery;
pub mod http;
pub mod llm;
pub mod metadata;
pub mod probe;
pub mod slskd;
#[cfg(test)]
pub(crate) mod testing;
pub mod youtube;
