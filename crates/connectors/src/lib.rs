//! Network and credential boundaries. The domain crate stays offline.
pub mod config;
pub mod credentials;
pub mod discovery;
pub mod http;
pub mod llm;
pub mod probe;
#[cfg(test)]
pub(crate) mod testing;
pub mod youtube;
