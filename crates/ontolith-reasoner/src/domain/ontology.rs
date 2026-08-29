//! Ontology payload linkage (P1-01): materialize the graph payload of a
//! Knowledge Object `OntologyObject` (SAS-0401 §9) for the reasoner.
//!
//! An `OntologyObject` is a specialized dataset that references role graphs
//! (tbox/abox/annotation/rule/provenance). [`load_ontology_payload`] reads
//! those referenced graphs through an [`OntologyGraphReader`] and returns the
//! triples split by role — the "载荷" (payload) that ties the KO container to
//! the forward-chaining pipeline.

use ontolith_core::domain::{GraphId, OntologyObject};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Triple;

/// Role-split triple payload materialized from an `OntologyObject`'s graph
/// references. Each role holds the decoded triples of the corresponding
/// graph (empty when the ontology does not reference that role).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OntologyPayload {
    /// Schema / axiom triples (rdfs:subClassOf, owl:* axioms, …).
    pub tbox: Vec<Triple>,
    /// Instance triples (asserted data).
    pub abox: Vec<Triple>,
    /// Annotation triples (labels, comments, provenance annotations).
    pub annotation: Vec<Triple>,
    /// Rule triples (rule graph payload).
    pub rule: Vec<Triple>,
    /// Provenance triples.
    pub provenance: Vec<Triple>,
}

impl OntologyPayload {
    pub fn is_empty(&self) -> bool {
        self.tbox.is_empty()
            && self.abox.is_empty()
            && self.annotation.is_empty()
            && self.rule.is_empty()
            && self.provenance.is_empty()
    }

    /// All payload triples in a stable role order (tbox, abox, annotation,
    /// rule, provenance). Duplicates across roles are preserved so the
    /// caller sees exactly what the graphs contain.
    pub fn all(&self) -> Vec<Triple> {
        let mut out = Vec::with_capacity(
            self.tbox.len()
                + self.abox.len()
                + self.annotation.len()
                + self.rule.len()
                + self.provenance.len(),
        );
        out.extend_from_slice(&self.tbox);
        out.extend_from_slice(&self.abox);
        out.extend_from_slice(&self.annotation);
        out.extend_from_slice(&self.rule);
        out.extend_from_slice(&self.provenance);
        out
    }
}

/// Reader of graph payloads. Implemented by storage/query read services
/// (see `ontolith-server::reasoning::QueryReadOntologyReader`).
pub trait OntologyGraphReader: Send + Sync {
    /// Decoded triples of `graph` (named graph or default graph).
    fn graph_triples(&self, graph: &GraphId) -> Result<Vec<Triple>, OntolithError>;
}

/// Materialize the payload of `ontology` through `reader`.
pub fn load_ontology_payload(
    reader: &dyn OntologyGraphReader,
    ontology: &OntologyObject,
) -> Result<OntologyPayload, OntolithError> {
    Ok(OntologyPayload {
        tbox: load_role(reader, ontology.tbox_graph.as_ref())?,
        abox: load_role(reader, ontology.abox_graph.as_ref())?,
        annotation: load_role(reader, ontology.annotation_graph.as_ref())?,
        rule: load_role(reader, ontology.rule_graph.as_ref())?,
        provenance: load_role(reader, ontology.provenance_graph.as_ref())?,
    })
}

fn load_role(
    reader: &dyn OntologyGraphReader,
    graph: Option<&GraphId>,
) -> Result<Vec<Triple>, OntolithError> {
    match graph {
        None => Ok(Vec::new()),
        Some(graph) => reader.graph_triples(graph),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::{Iri, NodeId, ObjectId, TimestampMs};
    use ontolith_rdf::domain::Term;
    use std::sync::Mutex;

    fn node_id(s: &str) -> NodeId {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in s.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        NodeId::new(hash)
    }

    fn triple(s: &str, p: &str, o: &str) -> Triple {
        Triple::new(node_id(s), Iri::new(p), Term::Iri(Iri::new(o)))
    }

    /// In-memory reader with explicit named/default graph contents.
    struct StubReader {
        named: Vec<(String, Vec<Triple>)>,
        default: Vec<Triple>,
    }

    impl OntologyGraphReader for StubReader {
        fn graph_triples(&self, graph: &GraphId) -> Result<Vec<Triple>, OntolithError> {
            match graph {
                GraphId::Default => Ok(self.default.clone()),
                GraphId::Named(iri) => Ok(self
                    .named
                    .iter()
                    .find(|(name, _)| name == iri.as_str())
                    .map(|(_, triples)| triples.clone())
                    .unwrap_or_default()),
            }
        }
    }

    fn ontology_with(graphs: &[(&str, GraphId)]) -> OntologyObject {
        let mut ontology = OntologyObject::new(
            ObjectId::new("ontology:test").expect("object id"),
            TimestampMs::default(),
        )
        .expect("ontology");
        for (role, graph) in graphs {
            match *role {
                "tbox" => ontology.tbox_graph = Some(graph.clone()),
                "abox" => ontology.abox_graph = Some(graph.clone()),
                "annotation" => ontology.annotation_graph = Some(graph.clone()),
                "rule" => ontology.rule_graph = Some(graph.clone()),
                "provenance" => ontology.provenance_graph = Some(graph.clone()),
                _ => unreachable!(),
            }
        }
        ontology
    }

    #[test]
    fn payload_splits_roles_and_merges_stably() {
        let tbox = vec![triple("urn:A", "rdfs:subClassOf", "urn:B")];
        let abox = vec![triple("urn:x", "rdf:type", "urn:A")];
        let annotation = vec![triple("urn:A", "rdfs:label", "urn:A-label")];
        let reader = StubReader {
            named: vec![
                ("urn:onto:tbox".into(), tbox.clone()),
                ("urn:onto:abox".into(), abox.clone()),
                ("urn:onto:anno".into(), annotation.clone()),
            ],
            default: Vec::new(),
        };
        let ontology = ontology_with(&[
            ("tbox", GraphId::named(Iri::new("urn:onto:tbox"))),
            ("abox", GraphId::named(Iri::new("urn:onto:abox"))),
            ("annotation", GraphId::named(Iri::new("urn:onto:anno"))),
        ]);
        let payload = load_ontology_payload(&reader, &ontology).expect("load");
        assert_eq!(payload.tbox, tbox);
        assert_eq!(payload.abox, abox);
        assert_eq!(payload.annotation, annotation);
        assert!(payload.rule.is_empty());
        assert!(payload.provenance.is_empty());
        assert_eq!(
            payload.all(),
            [tbox[0].clone(), abox[0].clone(), annotation[0].clone()]
        );
    }

    #[test]
    fn missing_roles_and_default_graph() {
        let default = vec![triple("urn:s", "urn:p", "urn:o")];
        let reader = StubReader {
            named: Vec::new(),
            default: default.clone(),
        };
        let ontology = ontology_with(&[("tbox", GraphId::Default)]);
        let payload = load_ontology_payload(&reader, &ontology).expect("load");
        assert_eq!(payload.tbox, default);
        assert!(payload.abox.is_empty());
        assert!(!payload.is_empty());
    }

    #[test]
    fn empty_ontology_payload() {
        let reader = StubReader {
            named: Vec::new(),
            default: Vec::new(),
        };
        let ontology = ontology_with(&[]);
        let payload = load_ontology_payload(&reader, &ontology).expect("load");
        assert!(payload.is_empty());
        assert!(payload.all().is_empty());
    }

    #[test]
    fn payload_is_send_sync_via_reader_trait() {
        let _: Mutex<()> = Mutex::new(());
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OntologyPayload>();
    }
}
