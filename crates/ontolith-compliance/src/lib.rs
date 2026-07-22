//! Ontolith R1 compliance harness helpers.
//!
//! Full W3C SPARQL 1.1 / RDF test suites are Phase 7 gate work. This crate
//! hosts a **curated smoke profile** that must stay green in CI and documents
//! the subset claimed for R1.

pub const CRATE_ID: &str = "ontolith-compliance";
pub const PROFILE: &str = "R1-smoke";
pub const W3C_SUBSET_PROFILE: &str = "W3C-subset-v0";

/// Human-readable list of SPARQL features covered by the smoke profile.
pub const SPARQL_R1_SMOKE_FEATURES: &[&str] = &[
    "SELECT * / projection",
    "ASK",
    "CONSTRUCT",
    "BGP multi-pattern JOIN",
    "OPTIONAL",
    "UNION",
    "FILTER (bound compare)",
    "BIND",
    "VALUES",
    "PREFIX",
    "LIMIT / OFFSET",
    "DISTINCT",
    "ORDER BY",
    "timeout / cancel hooks (engine-level)",
    "N-Triples / Turtle ingest → query",
];

/// Feature tags covered by the current W3C-inspired subset harness.
pub const SPARQL_W3C_SUBSET_FEATURES: &[&str] = &[
    "SELECT BGP",
    "ASK",
    "CONSTRUCT",
    "OPTIONAL",
    "UNION",
    "FILTER(BOUND)",
    "BIND",
    "VALUES",
    "Property path sequence (iri/iri baseline)",
    "Property path +/*/|/^ (minimal set baseline)",
    "Aggregate COUNT",
    "Subquery (nested SELECT + LIMIT baseline)",
    "DISTINCT/ORDER BY/LIMIT",
    "PREFIX + Turtle ingest",
    "unsupported: update",
];

pub fn profile_name() -> &'static str {
    PROFILE
}

pub fn w3c_subset_profile_name() -> &'static str {
    W3C_SUBSET_PROFILE
}
