//! Core domain primitives for Ontolith.
//!
//! Layer responsibility (SAS-0001 / SAS-0401):
//! - identity and lifecycle of Knowledge Objects
//! - resource vocabulary (IRI / blank / literal)
//! - canonical encoding helpers
//! - graph / dataset / ontology object headers
//!
//! Statement (Triple/Quad) value types remain in `ontolith-rdf` so the RDF
//! crate can evolve serialization independently while still depending on the
//! stable identity and resource types defined here.

mod canonical;
mod consistency;
mod identity;
mod knowledge;
mod resource;

pub use canonical::{CanonicalEncode, CanonicalWriter};
pub use consistency::ConsistencyLevel;
pub use identity::{
    KnowledgeObjectHeader, ObjectId, ObjectState, ObjectType, ObjectVersion, VersionId,
};
pub use knowledge::{
    DatasetObject, GraphId, GraphObject, GraphStatistics, ObjectMetadata, OntologyObject,
    RuleObject, VersionObject,
};
pub use resource::{BlankNodeId, BoundResource, Iri, LanguageTag, Literal, LiteralValue, Resource};

/// Stable internal node identifier assigned by the dictionary layer.
///
/// Node identifiers SHALL remain immutable throughout the lifetime of the
/// database epoch (SAS-0401 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl CanonicalEncode for NodeId {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"N");
        out.write_u64(self.0);
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node:{}", self.0)
    }
}

/// Milliseconds since UNIX epoch (UTC). Wall-clock source is chosen by callers.
pub type TimestampMs = u64;

pub fn status() -> &'static str {
    "domain"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OntolithError;

    #[test]
    fn object_id_rejects_empty() {
        assert!(ObjectId::new("").is_err());
        assert_eq!(ObjectId::new("ko:1").unwrap().as_str(), "ko:1");
    }

    #[test]
    fn lifecycle_allows_normative_path_and_delete() {
        let id = ObjectId::new("ko:graph:1").unwrap();
        let mut header = KnowledgeObjectHeader::new(id, ObjectType::Graph, 1000);
        assert_eq!(header.state, ObjectState::Created);
        assert_eq!(header.version, ObjectVersion::INITIAL);

        header.transition_to(ObjectState::Persisted, 1001).unwrap();
        header.transition_to(ObjectState::Indexed, 1002).unwrap();
        header.transition_to(ObjectState::Replicated, 1003).unwrap();
        header.transition_to(ObjectState::Versioned, 1004).unwrap();
        assert!(header.version.get() >= 5);

        // Logical delete from versioned is allowed.
        header.transition_to(ObjectState::Deleted, 1005).unwrap();
        assert_eq!(header.state, ObjectState::Deleted);

        // No transitions out of deleted.
        assert!(header.transition_to(ObjectState::Persisted, 1006).is_err());
    }

    #[test]
    fn lifecycle_rejects_skip_ahead() {
        let id = ObjectId::new("ko:x").unwrap();
        let mut header = KnowledgeObjectHeader::new(id, ObjectType::Resource, 1);
        assert!(header.transition_to(ObjectState::Indexed, 2).is_err());
    }

    #[test]
    fn iri_parse_baseline() {
        assert!(Iri::parse("http://example.org/a").is_ok());
        assert!(Iri::parse("").is_err());
        assert!(Iri::parse("no-scheme").is_err());
        assert!(Iri::parse("http://example.org/a b").is_err());
    }

    #[test]
    fn resource_canonical_encoding_is_stable() {
        let iri = Resource::iri("urn:ex:alice").unwrap();
        let again = Resource::iri("urn:ex:alice").unwrap();
        assert_eq!(iri.canonical_bytes(), again.canonical_bytes());

        let lit = Resource::literal(Literal::string("hello"));
        let lit2 = Resource::literal(Literal::string("hello"));
        assert_eq!(lit.canonical_bytes(), lit2.canonical_bytes());
        assert_ne!(iri.canonical_bytes(), lit.canonical_bytes());
    }

    #[test]
    fn language_tag_normalizes_case() {
        let tag = LanguageTag::parse("EN-us").unwrap();
        assert_eq!(tag.as_str(), "en-us");
    }

    #[test]
    fn dataset_manages_named_graphs() {
        let ds_id = ObjectId::new("dataset:main").unwrap();
        let mut dataset = DatasetObject::new(ds_id, 10).unwrap();
        assert_eq!(dataset.graph_count(), 1);

        let g_id = ObjectId::new("dataset:main/graph/people").unwrap();
        let graph = GraphObject::new_named(g_id, Iri::new("urn:graph:people"), 10);
        dataset.add_named_graph(graph).unwrap();
        assert_eq!(dataset.graph_count(), 2);
        assert!(dataset.named_graph(&Iri::new("urn:graph:people")).is_some());

        let dup = GraphObject::new_named(
            ObjectId::new("dataset:main/graph/people-2").unwrap(),
            Iri::new("urn:graph:people"),
            11,
        );
        assert!(matches!(
            dataset.add_named_graph(dup),
            Err(OntolithError::AlreadyExists(_))
        ));
    }

    #[test]
    fn ontology_is_specialized_dataset() {
        let id = ObjectId::new("ontology:base").unwrap();
        let ontology = OntologyObject::new(id, 42).unwrap();
        assert_eq!(ontology.header().object_type, ObjectType::Ontology);
        assert_eq!(ontology.header().state, ObjectState::Created);
    }

    #[test]
    fn metadata_canonical_ignores_insertion_order() {
        let mut a = ObjectMetadata::new();
        a.insert("b", "2");
        a.insert("a", "1");

        let mut b = ObjectMetadata::new();
        b.insert("a", "1");
        b.insert("b", "2");

        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn node_id_display_and_canonical() {
        let id = NodeId::new(7);
        assert_eq!(id.to_string(), "node:7");
        assert_eq!(id.canonical_bytes(), NodeId::new(7).canonical_bytes());
        assert_ne!(id.canonical_bytes(), NodeId::new(8).canonical_bytes());
    }

    #[test]
    fn error_display_includes_code() {
        let err = OntolithError::Unsupported("sparql-update");
        assert_eq!(err.code(), "unsupported");
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn consistency_level_flags() {
        assert!(ConsistencyLevel::Strong.requires_primary());
        assert!(!ConsistencyLevel::Eventual.requires_primary());
        assert_eq!(ConsistencyLevel::Session.as_str(), "session");
    }
}
