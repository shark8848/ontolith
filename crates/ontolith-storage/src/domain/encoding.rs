//! Physical / index key encoding for the storage layer (L2).
//!
//! Keys are deterministic byte sequences built with
//! [`ontolith_core::domain::CanonicalWriter`]. They are backend-agnostic:
//! the same bytes can later be used as RocksDB keys without leaking vendor APIs.

use ontolith_core::domain::{CanonicalEncode, CanonicalWriter, Iri, NodeId};
use ontolith_rdf::domain::{Term, Triple};

/// Index permutation maintained by the storage engine.
///
/// R1 baseline requires at least SPO / POS / OSP (PLAN-0001 Phase 2).
/// SOP / PSO / OPS are reserved for future coverage of full 6-permutation
/// plans described in SAS-0001 §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexKind {
    Spo,
    Pos,
    Osp,
    Sop,
    Pso,
    Ops,
}

impl IndexKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spo => "spo",
            Self::Pos => "pos",
            Self::Osp => "osp",
            Self::Sop => "sop",
            Self::Pso => "pso",
            Self::Ops => "ops",
        }
    }

    /// Indexes required for R1 correctness of basic triple pattern matching.
    pub const fn r1_required(self) -> bool {
        matches!(self, Self::Spo | Self::Pos | Self::Osp)
    }
}

impl std::fmt::Display for IndexKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Encode a full SPO index key: tag || S || P || O.
pub fn encode_spo_key(subject: NodeId, predicate: &Iri, object: &Term) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(64);
    out.write_tag(b"SPO");
    out.write_u64(subject.get());
    out.write_str(predicate.as_str());
    object.write_canonical(&mut out);
    out.into_bytes()
}

/// Encode a full POS index key: tag || P || O || S.
pub fn encode_pos_key(predicate: &Iri, object: &Term, subject: NodeId) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(64);
    out.write_tag(b"POS");
    out.write_str(predicate.as_str());
    object.write_canonical(&mut out);
    out.write_u64(subject.get());
    out.into_bytes()
}

/// Encode a full OSP index key: tag || O || S || P.
pub fn encode_osp_key(object: &Term, subject: NodeId, predicate: &Iri) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(64);
    out.write_tag(b"OSP");
    object.write_canonical(&mut out);
    out.write_u64(subject.get());
    out.write_str(predicate.as_str());
    out.into_bytes()
}

/// Encode the primary key for a triple under the given index kind.
pub fn encode_triple_index_key(kind: IndexKind, triple: &Triple) -> Vec<u8> {
    match kind {
        IndexKind::Spo => encode_spo_key(triple.subject, &triple.predicate, &triple.object),
        IndexKind::Pos => encode_pos_key(&triple.predicate, &triple.object, triple.subject),
        IndexKind::Osp => encode_osp_key(&triple.object, triple.subject, &triple.predicate),
        // Reserved permutations: still deterministic, not yet maintained in engine.
        IndexKind::Sop => {
            let mut out = CanonicalWriter::with_capacity(64);
            out.write_tag(b"SOP");
            out.write_u64(triple.subject.get());
            triple.object.write_canonical(&mut out);
            out.write_str(triple.predicate.as_str());
            out.into_bytes()
        }
        IndexKind::Pso => {
            let mut out = CanonicalWriter::with_capacity(64);
            out.write_tag(b"PSO");
            out.write_str(triple.predicate.as_str());
            out.write_u64(triple.subject.get());
            triple.object.write_canonical(&mut out);
            out.into_bytes()
        }
        IndexKind::Ops => {
            let mut out = CanonicalWriter::with_capacity(64);
            out.write_tag(b"OPS");
            triple.object.write_canonical(&mut out);
            out.write_str(triple.predicate.as_str());
            out.write_u64(triple.subject.get());
            out.into_bytes()
        }
    }
}

/// Prefix key for lookup by subject under SPO (all predicates/objects).
pub fn encode_spo_subject_prefix(subject: NodeId) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(16);
    out.write_tag(b"SPO");
    out.write_u64(subject.get());
    out.into_bytes()
}

/// Prefix key for lookup by predicate under POS.
pub fn encode_pos_predicate_prefix(predicate: &Iri) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(32);
    out.write_tag(b"POS");
    out.write_str(predicate.as_str());
    out.into_bytes()
}

/// Prefix key for lookup by object under OSP.
pub fn encode_osp_object_prefix(object: &Term) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(32);
    out.write_tag(b"OSP");
    object.write_canonical(&mut out);
    out.into_bytes()
}

/// Dictionary entry encoding: maps lexical form to node id for durable dict.
pub fn encode_dictionary_entry(value: &str, node_id: NodeId) -> Vec<u8> {
    let mut out = CanonicalWriter::with_capacity(value.len() + 16);
    out.write_tag(b"DICT");
    out.write_str(value);
    out.write_u64(node_id.get());
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_rdf::domain::Term;

    fn sample_triple() -> Triple {
        Triple::new(NodeId::new(1), Iri::new("urn:p"), Term::iri("urn:o"))
    }

    #[test]
    fn spo_pos_osp_keys_differ_and_are_stable() {
        let t = sample_triple();
        let spo1 = encode_triple_index_key(IndexKind::Spo, &t);
        let spo2 = encode_triple_index_key(IndexKind::Spo, &t);
        let pos = encode_triple_index_key(IndexKind::Pos, &t);
        let osp = encode_triple_index_key(IndexKind::Osp, &t);
        assert_eq!(spo1, spo2);
        assert_ne!(spo1, pos);
        assert_ne!(spo1, osp);
        assert_ne!(pos, osp);
    }

    #[test]
    fn prefixes_are_prefixes_of_full_keys() {
        let t = sample_triple();
        let spo = encode_triple_index_key(IndexKind::Spo, &t);
        let spo_prefix = encode_spo_subject_prefix(t.subject);
        assert!(spo.starts_with(&spo_prefix));

        let pos = encode_triple_index_key(IndexKind::Pos, &t);
        let pos_prefix = encode_pos_predicate_prefix(&t.predicate);
        assert!(pos.starts_with(&pos_prefix));

        let osp = encode_triple_index_key(IndexKind::Osp, &t);
        let osp_prefix = encode_osp_object_prefix(&t.object);
        assert!(osp.starts_with(&osp_prefix));
    }

    #[test]
    fn r1_required_flags() {
        assert!(IndexKind::Spo.r1_required());
        assert!(IndexKind::Pos.r1_required());
        assert!(IndexKind::Osp.r1_required());
        assert!(!IndexKind::Sop.r1_required());
    }
}
