//! Physical key encoders for durable storage records.
//!
//! Keys are deterministic byte sequences shared by the in-memory and RocksDB
//! engines (RFC-0001 §4). Index column-family reads use the same key bytes.

use ontolith_core::domain::{CanonicalEncode, Iri};
use ontolith_rdf::domain::{Quad, Triple};

/// Canonical equality key for a default-graph triple.
pub fn triple_key(t: &Triple) -> Vec<u8> {
    let mut out = ontolith_core::domain::CanonicalWriter::with_capacity(64);
    out.write_tag(b"TK");
    out.write_u64(t.subject.get());
    out.write_str(t.predicate.as_str());
    t.object.write_canonical(&mut out);
    out.into_bytes()
}

pub fn quad_key(q: &Quad) -> Vec<u8> {
    let mut out = ontolith_core::domain::CanonicalWriter::with_capacity(80);
    out.write_tag(b"QK");
    match &q.graph_name {
        None => out.write_tag(b"GD"),
        Some(g) => {
            out.write_tag(b"GN");
            out.write_str(g.as_str());
        }
    }
    out.write_u64(q.triple.subject.get());
    out.write_str(q.triple.predicate.as_str());
    q.triple.object.write_canonical(&mut out);
    out.into_bytes()
}

/// Prefix key for all quads in a named graph (matches [`quad_key`] layout).
pub fn quad_graph_prefix(graph_name: &Iri) -> Vec<u8> {
    let mut out = ontolith_core::domain::CanonicalWriter::with_capacity(48);
    out.write_tag(b"QK");
    out.write_tag(b"GN");
    out.write_str(graph_name.as_str());
    out.into_bytes()
}
