//! Ontolith RDF parser (L3).
//!
//! Formats: N-Triples, N-Quads, Turtle, TriG, and a pragmatic JSON-LD 1.0/1.1
//! subset (see [`infrastructure::json_ld`] for the supported `@context`
//! surface). All declared [`domain::ParseFormat`] variants are implemented.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-parser";
pub const LAYER: &str = "L3-parser-query";

pub fn healthcheck() -> bool {
    true
}
