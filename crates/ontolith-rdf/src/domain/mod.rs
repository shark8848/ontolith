//! RDF domain model (L1).
//!
//! Builds on `ontolith-core` Knowledge Object primitives and provides the
//! statement / graph / dataset value types used by storage, parser, and query.

mod graph;
mod statement;
mod term;

pub use graph::{Dataset, NamedGraph, graph_id_from_option};
pub use statement::{Quad, StatementObject, Triple};
pub use term::{Predicate, Subject, Term};

pub fn status() -> &'static str {
    "domain"
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::{CanonicalEncode, Iri, LiteralValue, NodeId, ObjectId, ObjectType};
    use ontolith_core::error::OntolithError;

    fn sample_triple(s: u64, p: &str, o: &str) -> Triple {
        Triple::new(NodeId::new(s), Iri::new(p), Term::Iri(Iri::new(o)))
    }

    #[test]
    fn triple_canonical_is_stable_and_order_sensitive() {
        let a = sample_triple(1, "urn:p", "urn:o");
        let b = sample_triple(1, "urn:p", "urn:o");
        let c = sample_triple(2, "urn:p", "urn:o");
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
        assert_ne!(a.canonical_bytes(), c.canonical_bytes());
    }

    #[test]
    fn triple_validation_rejects_bad_predicate() {
        let bad = Triple::new(
            NodeId::new(1),
            Iri::new("not-an-iri"),
            Term::literal(LiteralValue::Integer(1)),
        );
        assert!(matches!(
            bad.validated(),
            Err(OntolithError::InvalidArgument(_))
        ));
    }

    #[test]
    fn quad_default_and_named_graph_ids() {
        let triple = sample_triple(1, "urn:p", "urn:o");
        let q0 = Quad::in_default_graph(triple.clone());
        assert!(q0.graph_id().is_default());

        let q1 = Quad::in_named_graph(triple, Iri::new("urn:g"));
        assert!(!q1.graph_id().is_default());
    }

    #[test]
    fn dataset_insert_and_quad_roundtrip() {
        let mut ds = Dataset::new();
        ds.insert_default(sample_triple(1, "urn:p", "urn:o1"));
        ds.insert_named(Iri::new("urn:g"), sample_triple(2, "urn:p", "urn:o2"));
        ds.insert_quad(Quad::in_named_graph(
            sample_triple(2, "urn:p", "urn:o3"),
            Iri::new("urn:g"),
        ));

        assert_eq!(ds.graph_count(), 2);
        assert_eq!(ds.triple_count(), 3);
        assert_eq!(ds.named_graph(&Iri::new("urn:g")).unwrap().len(), 2);
        assert_eq!(ds.quads().len(), 3);
    }

    #[test]
    fn dataset_canonical_ignores_triple_insertion_order() {
        let mut a = Dataset::new();
        a.insert_default(sample_triple(1, "urn:p", "urn:o1"));
        a.insert_default(sample_triple(2, "urn:p", "urn:o2"));

        let mut b = Dataset::new();
        b.insert_default(sample_triple(2, "urn:p", "urn:o2"));
        b.insert_default(sample_triple(1, "urn:p", "urn:o1"));

        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn dataset_canonical_sorts_named_graphs_by_name() {
        let mut a = Dataset::new();
        a.insert_named(Iri::new("urn:g:b"), sample_triple(1, "urn:p", "urn:o"));
        a.insert_named(Iri::new("urn:g:a"), sample_triple(1, "urn:p", "urn:o"));

        let mut b = Dataset::new();
        b.insert_named(Iri::new("urn:g:a"), sample_triple(1, "urn:p", "urn:o"));
        b.insert_named(Iri::new("urn:g:b"), sample_triple(1, "urn:p", "urn:o"));

        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn dataset_bridges_to_core_dataset_object() {
        let mut ds = Dataset::new();
        ds.insert_default(sample_triple(1, "urn:p", "urn:o"));
        ds.insert_named(Iri::new("urn:g"), sample_triple(2, "urn:p", "urn:o"));

        let object = ds
            .to_dataset_object(ObjectId::new("dataset:demo").unwrap(), 100)
            .unwrap();
        assert_eq!(object.header.object_type, ObjectType::Dataset);
        assert_eq!(object.graph_count(), 2);
        assert_eq!(object.default_graph.statistics.triple_count, 1);
        assert_eq!(
            object
                .named_graph(&Iri::new("urn:g"))
                .unwrap()
                .statistics
                .triple_count,
            1
        );
    }

    #[test]
    fn statement_object_wraps_triple() {
        let triple = sample_triple(9, "urn:p", "urn:o");
        let stmt =
            StatementObject::from_triple(ObjectId::new("stmt:1").unwrap(), triple, 1).unwrap();
        assert_eq!(stmt.header.object_type, ObjectType::Statement);
        assert!(stmt.quad.graph_name.is_none());
    }

    #[test]
    fn term_kinds_and_resource_projection() {
        let iri = Term::iri("urn:x");
        assert_eq!(iri.kind(), "iri");
        assert!(matches!(
            iri.to_resource(),
            ontolith_core::domain::Resource::Iri(_)
        ));

        let blank = Term::blank(NodeId::new(3));
        assert_eq!(blank.kind(), "blank_node");

        let lit = Term::literal(LiteralValue::Boolean(true));
        assert_eq!(lit.kind(), "literal");
    }

    #[test]
    fn statistics_count_distincts() {
        let triples = vec![
            sample_triple(1, "urn:p1", "urn:o1"),
            sample_triple(1, "urn:p2", "urn:o1"),
            sample_triple(2, "urn:p1", "urn:o2"),
        ];
        let graph = NamedGraph::with_triples(Iri::new("urn:g"), triples);
        let stats = graph.statistics();
        assert_eq!(stats.triple_count, 3);
        assert_eq!(stats.distinct_subjects, 2);
        assert_eq!(stats.distinct_predicates, 2);
        assert_eq!(stats.distinct_objects, 2);
    }
}
