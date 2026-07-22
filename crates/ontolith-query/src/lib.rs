//! Ontolith query engine (L3 — full SPARQL core).
//!
//! Pipeline: SPARQL text → parse → algebra → rule optimize → physical execute
//! → solution bindings / ASK boolean / CONSTRUCT triples.
//!
//! Supports SELECT, ASK, CONSTRUCT with WHERE groups, OPTIONAL, UNION, FILTER,
//! BIND, VALUES, DISTINCT, ORDER BY, LIMIT/OFFSET, PREFIX/BASE, timeout and
//! cooperative cancel. Storage access uses SPO/POS/OSP indexes from L2.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-query";
pub const LAYER: &str = "L3-parser-query";

pub fn healthcheck() -> bool {
    true
}
