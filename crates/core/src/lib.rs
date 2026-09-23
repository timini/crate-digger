//! Crate Digger core: domain model, persistence, jobs and library logic.
//!
//! Nothing in this crate touches the network. External systems are reached
//! only through the traits in [`adapters`].

pub mod adapters;
pub mod db;
pub mod domain;
pub mod error;
pub mod meta;
pub mod util;

pub use error::{Error, Result};
