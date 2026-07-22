//! Ontolith RDF parser (L3).
//!
//! Baseline formats: N-Triples and N-Quads. Turtle / TriG / JSON-LD remain
//! declared on [`domain::ParseFormat`] but return `Unsupported` until implemented.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-parser";
pub const LAYER: &str = "L3-parser-query";

pub fn healthcheck() -> bool {
    true
}
