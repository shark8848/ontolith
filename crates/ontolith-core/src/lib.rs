//! Ontolith core domain library.
//!
//! This crate is the lowest architectural layer (SAS-0001 / SAS-0401):
//! shared identity, resource vocabulary, Knowledge Object headers, errors,
//! and canonical encoding helpers. Higher crates (`ontolith-rdf`,
//! `ontolith-storage`, …) depend on these contracts and must not redefine them.

pub mod application;
pub mod domain;
pub mod error;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-core";
pub const LAYER: &str = "L0-core-knowledge-object";

pub fn healthcheck() -> bool {
    true
}
