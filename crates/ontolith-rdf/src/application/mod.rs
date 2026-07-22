//! Application services for RDF value manipulation (L1).
//!
//! Keep this layer free of storage I/O — only pure transforms on datasets.

use ontolith_core::domain::{DatasetObject, ObjectId, TimestampMs};
use ontolith_core::error::OntolithError;

use crate::domain::{Dataset, Quad, Triple};

/// Pure helpers for building and inspecting datasets.
pub struct DatasetService;

impl DatasetService {
    pub fn empty() -> Dataset {
        Dataset::new()
    }

    pub fn from_triples(triples: Vec<Triple>) -> Dataset {
        let mut dataset = Dataset::new();
        for triple in triples {
            dataset.insert_default(triple);
        }
        dataset
    }

    pub fn from_quads(quads: Vec<Quad>) -> Dataset {
        let mut dataset = Dataset::new();
        for quad in quads {
            dataset.insert_quad(quad);
        }
        dataset
    }

    pub fn to_knowledge_object(
        dataset: &Dataset,
        object_id: ObjectId,
        created_at: TimestampMs,
    ) -> Result<DatasetObject, OntolithError> {
        dataset.to_dataset_object(object_id, created_at)
    }

    pub fn merge(into: &mut Dataset, other: Dataset) {
        into.merge(other);
    }
}

pub fn status() -> &'static str {
    "application"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Term;
    use ontolith_core::domain::{Iri, NodeId};

    #[test]
    fn service_builds_dataset_from_quads() {
        let quads = vec![
            Quad::in_default_graph(Triple::new(
                NodeId::new(1),
                Iri::new("urn:p"),
                Term::iri("urn:o"),
            )),
            Quad::in_named_graph(
                Triple::new(NodeId::new(2), Iri::new("urn:p"), Term::iri("urn:o")),
                Iri::new("urn:g"),
            ),
        ];
        let ds = DatasetService::from_quads(quads);
        assert_eq!(ds.triple_count(), 2);
        assert_eq!(ds.graph_count(), 2);
    }
}
