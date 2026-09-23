//! Crate Digger core: domain model, persistence, jobs and library logic.
//!
//! Nothing in this crate touches the network. External systems are reached
//! only through the traits in [`adapters`].

// Lets shared test support code refer to this crate by name.
#[cfg(test)]
extern crate self as cd_core;

pub mod acquisition;
pub mod adapters;
pub mod analysis;
pub mod archive;
pub mod db;
pub mod discovery;
pub mod domain;
pub mod error;
pub mod identity;
pub mod jobs;
pub mod library;
pub mod meta;
pub mod pipeline;
pub mod playlists;
pub mod review;
pub mod settings;
pub mod util;

pub use error::{Error, Result};

#[cfg(test)]
#[path = "../test_support/real_probe.rs"]
pub(crate) mod real_probe;
