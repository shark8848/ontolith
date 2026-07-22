//! Ontolith RDF value model (L1).
//!
//! Provides Triple/Quad/Graph/Dataset types on top of `ontolith-core`
//! Knowledge Object primitives. Parsing of concrete RDF syntaxes lives in
//! `ontolith-parser`; durable storage lives in `ontolith-storage`.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-rdf";
pub const LAYER: &str = "L1-rdf-statement-model";

pub fn healthcheck() -> bool {
    true
}
