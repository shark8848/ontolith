//! Knowledge Object containers: Graph / Dataset / Ontology / Metadata / Rule
//! (SAS-0401 §3, §7–§9). Statement (Triple/Quad) remains in `ontolith-rdf`
//! and references these identity/resource primitives via `NodeId` / `Iri`.

use crate::domain::TimestampMs;
use crate::domain::canonical::{CanonicalEncode, CanonicalWriter};
use crate::domain::identity::{
    KnowledgeObjectHeader, ObjectId, ObjectState, ObjectType, ObjectVersion,
};
use crate::domain::resource::Iri;
use crate::error::OntolithError;

/// Lightweight metadata bag attached to Knowledge Objects.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjectMetadata {
    pub labels: Vec<(String, String)>,
}

impl ObjectMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        if let Some((_, existing)) = self.labels.iter_mut().find(|(k, _)| k == &key) {
            *existing = value.into();
        } else {
            self.labels.push((key, value.into()));
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

impl CanonicalEncode for ObjectMetadata {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"M");
        out.write_u64(self.labels.len() as u64);
        // Sort for deterministic encoding regardless of insertion order.
        let mut pairs = self.labels.clone();
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for (k, v) in pairs {
            out.write_str(&k);
            out.write_str(&v);
        }
    }
}

/// Graph identifier: default graph or a named graph IRI.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum GraphId {
    #[default]
    Default,
    Named(Iri),
}

impl GraphId {
    pub fn named(iri: Iri) -> Self {
        Self::Named(iri)
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}

impl CanonicalEncode for GraphId {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        match self {
            Self::Default => out.write_tag(b"GD"),
            Self::Named(iri) => {
                out.write_tag(b"GN");
                out.write_str(iri.as_str());
            }
        }
    }
}

/// Aggregate statistics for a graph (SAS-0401 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphStatistics {
    pub triple_count: u64,
    pub distinct_subjects: u64,
    pub distinct_predicates: u64,
    pub distinct_objects: u64,
}

/// Graph Knowledge Object header + identity (statement payload lives in RDF layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphObject {
    pub header: KnowledgeObjectHeader,
    pub graph_id: GraphId,
    pub metadata: ObjectMetadata,
    pub statistics: GraphStatistics,
}

impl GraphObject {
    pub fn new_default(object_id: ObjectId, created_at: TimestampMs) -> Self {
        Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Graph, created_at),
            graph_id: GraphId::Default,
            metadata: ObjectMetadata::default(),
            statistics: GraphStatistics::default(),
        }
    }

    pub fn new_named(object_id: ObjectId, name: Iri, created_at: TimestampMs) -> Self {
        Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Graph, created_at),
            graph_id: GraphId::Named(name),
            metadata: ObjectMetadata::default(),
            statistics: GraphStatistics::default(),
        }
    }
}

/// Dataset: default graph + named graphs (SAS-0401 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetObject {
    pub header: KnowledgeObjectHeader,
    pub default_graph: GraphObject,
    pub named_graphs: Vec<GraphObject>,
    pub metadata: ObjectMetadata,
}

impl DatasetObject {
    pub fn new(object_id: ObjectId, created_at: TimestampMs) -> Result<Self, OntolithError> {
        let default_graph_id = ObjectId::new(format!("{}/graph/default", object_id.as_str()))
            .map_err(OntolithError::InvalidArgument)?;
        Ok(Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Dataset, created_at),
            default_graph: GraphObject::new_default(default_graph_id, created_at),
            named_graphs: Vec::new(),
            metadata: ObjectMetadata::default(),
        })
    }

    pub fn add_named_graph(&mut self, graph: GraphObject) -> Result<(), OntolithError> {
        if graph.graph_id.is_default() {
            return Err(OntolithError::InvalidArgument(
                "named graph entry must not use the default graph id",
            ));
        }
        if self
            .named_graphs
            .iter()
            .any(|existing| existing.graph_id == graph.graph_id)
        {
            return Err(OntolithError::AlreadyExists("named graph already present"));
        }
        self.named_graphs.push(graph);
        Ok(())
    }

    pub fn named_graph(&self, name: &Iri) -> Option<&GraphObject> {
        self.named_graphs.iter().find(|g| match &g.graph_id {
            GraphId::Named(iri) => iri == name,
            GraphId::Default => false,
        })
    }

    pub fn graph_count(&self) -> usize {
        1 + self.named_graphs.len()
    }
}

/// Ontology as a specialized Dataset (SAS-0401 §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OntologyObject {
    pub dataset: DatasetObject,
    pub tbox_graph: Option<GraphId>,
    pub abox_graph: Option<GraphId>,
    pub annotation_graph: Option<GraphId>,
    pub rule_graph: Option<GraphId>,
    pub provenance_graph: Option<GraphId>,
}

impl OntologyObject {
    pub fn new(object_id: ObjectId, created_at: TimestampMs) -> Result<Self, OntolithError> {
        let mut dataset = DatasetObject::new(object_id, created_at)?;
        dataset.header.object_type = ObjectType::Ontology;
        Ok(Self {
            dataset,
            tbox_graph: None,
            abox_graph: None,
            annotation_graph: None,
            rule_graph: None,
            provenance_graph: None,
        })
    }

    pub fn header(&self) -> &KnowledgeObjectHeader {
        &self.dataset.header
    }
}

/// Rule object placeholder for reasoner integration (SAS-0401 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleObject {
    pub header: KnowledgeObjectHeader,
    pub rule_iri: Option<Iri>,
    pub label: String,
}

impl RuleObject {
    pub fn new(object_id: ObjectId, label: impl Into<String>, created_at: TimestampMs) -> Self {
        Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Rule, created_at),
            rule_iri: None,
            label: label.into(),
        }
    }
}

/// Explicit version record for a Knowledge Object lineage (SAS-0401 §3 / §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionObject {
    pub header: KnowledgeObjectHeader,
    pub target_id: ObjectId,
    pub target_version: ObjectVersion,
    pub parent_version: Option<ObjectVersion>,
}

impl VersionObject {
    pub fn new(
        object_id: ObjectId,
        target_id: ObjectId,
        target_version: ObjectVersion,
        parent_version: Option<ObjectVersion>,
        created_at: TimestampMs,
    ) -> Self {
        let mut header = KnowledgeObjectHeader::new(object_id, ObjectType::Version, created_at);
        header.state = ObjectState::Versioned;
        Self {
            header,
            target_id,
            target_version,
            parent_version,
        }
    }
}
