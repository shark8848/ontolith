//! Infrastructure adapters for RDF (L1).
//!
//! No external format parsers here yet — those belong to `ontolith-parser`.
//! This module reserves the slot for future in-memory indexes or interning
//! helpers that stay local to the RDF crate.

pub fn status() -> &'static str {
    "infrastructure"
}
