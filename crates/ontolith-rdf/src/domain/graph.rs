//! Graph and Dataset value types + bridge to core Knowledge Objects.

use ontolith_core::domain::{
    CanonicalEncode, CanonicalWriter, DatasetObject, GraphId, GraphObject, GraphStatistics, Iri,
    ObjectId, ObjectMetadata, TimestampMs,
};
use ontolith_core::error::OntolithError;

use super::statement::{Quad, Triple};

/// Named graph: name + triples.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedGraph {
    pub name: Iri,
    pub triples: Vec<Triple>,
}

impl NamedGraph {
    pub fn new(name: Iri) -> Self {
        Self {
            name,
            triples: Vec::new(),
        }
    }

    pub fn with_triples(name: Iri, triples: Vec<Triple>) -> Self {
        Self { name, triples }
    }

    pub fn insert(&mut self, triple: Triple) {
        self.triples.push(triple);
    }

    /// Set-semantics insert (L1 §7 dedup follow-up): appends the triple only
    /// when no equal-by-canonical-bytes member exists. Returns `true` when the
    /// graph changed. Identity follows the canonical encoding, matching the
    /// object-dedup rule used by statistics.
    pub fn insert_unique(&mut self, triple: Triple) -> bool {
        let key = triple.canonical_bytes();
        if self.triples.iter().any(|t| t.canonical_bytes() == key) {
            return false;
        }
        self.triples.push(triple);
        true
    }

    /// True when an equal-by-canonical-bytes triple is present.
    pub fn contains(&self, triple: &Triple) -> bool {
        let key = triple.canonical_bytes();
        self.triples.iter().any(|t| t.canonical_bytes() == key)
    }

    /// Remove the first equal-by-canonical-bytes triple; returns it if present.
    pub fn remove(&mut self, triple: &Triple) -> Option<Triple> {
        let key = triple.canonical_bytes();
        let idx = self
            .triples
            .iter()
            .position(|t| t.canonical_bytes() == key)?;
        Some(self.triples.remove(idx))
    }

    /// Collapse duplicate members, keeping first occurrences in order.
    /// Returns the number of duplicates removed.
    pub fn dedup(&mut self) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        let before = self.triples.len();
        self.triples.retain(|t| seen.insert(t.canonical_bytes()));
        before - self.triples.len()
    }

    pub fn len(&self) -> usize {
        self.triples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Triple> {
        self.triples.iter()
    }

    pub fn to_quads(&self) -> Vec<Quad> {
        self.triples
            .iter()
            .cloned()
            .map(|t| Quad::in_named_graph(t, self.name.clone()))
            .collect()
    }

    pub fn statistics(&self) -> GraphStatistics {
        compute_triple_statistics(&self.triples)
    }

    /// Build a core [`GraphObject`] header companion for this named graph.
    pub fn to_graph_object(&self, object_id: ObjectId, created_at: TimestampMs) -> GraphObject {
        let mut graph = GraphObject::new_named(object_id, self.name.clone(), created_at);
        graph.statistics = self.statistics();
        graph
    }
}

impl CanonicalEncode for NamedGraph {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"NG");
        out.write_str(self.name.as_str());
        out.write_u64(self.triples.len() as u64);
        // Sort triples by canonical bytes for deterministic graph encoding.
        let mut encoded: Vec<Vec<u8>> = self
            .triples
            .iter()
            .map(CanonicalEncode::canonical_bytes)
            .collect();
        encoded.sort();
        for bytes in encoded {
            out.write_bytes(&bytes);
        }
    }
}

/// RDF Dataset: default graph + zero or more named graphs.
///
/// This is the logical exchange boundary for import/export (SAS-0401 §8).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dataset {
    pub default_graph: Vec<Triple>,
    pub named_graphs: Vec<NamedGraph>,
}

impl Dataset {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_default(&mut self, triple: Triple) {
        self.default_graph.push(triple);
    }

    pub fn insert_named(&mut self, graph_name: Iri, triple: Triple) {
        if let Some(graph) = self.named_graphs.iter_mut().find(|g| g.name == graph_name) {
            graph.insert(triple);
            return;
        }
        let mut graph = NamedGraph::new(graph_name);
        graph.insert(triple);
        self.named_graphs.push(graph);
    }

    pub fn insert_quad(&mut self, quad: Quad) {
        match quad.graph_name {
            None => self.insert_default(quad.triple),
            Some(name) => self.insert_named(name, quad.triple),
        }
    }

    /// Set-semantics insert into the default graph (see [`NamedGraph::insert_unique`]).
    pub fn insert_default_unique(&mut self, triple: Triple) -> bool {
        let key = triple.canonical_bytes();
        if self
            .default_graph
            .iter()
            .any(|t| t.canonical_bytes() == key)
        {
            return false;
        }
        self.default_graph.push(triple);
        true
    }

    /// Set-semantics insert into a named graph, creating the graph on first
    /// unique insert (see [`NamedGraph::insert_unique`]).
    pub fn insert_named_unique(&mut self, graph_name: Iri, triple: Triple) -> bool {
        if let Some(graph) = self.named_graphs.iter_mut().find(|g| g.name == graph_name) {
            return graph.insert_unique(triple);
        }
        let mut graph = NamedGraph::new(graph_name);
        graph.insert_unique(triple);
        // insert_unique on a fresh graph always succeeds
        let changed = !graph.is_empty();
        self.named_graphs.push(graph);
        changed
    }

    /// Set-semantics insert for a quad (default graph when `graph_name` is `None`).
    pub fn insert_quad_unique(&mut self, quad: Quad) -> bool {
        match quad.graph_name {
            None => self.insert_default_unique(quad.triple),
            Some(name) => self.insert_named_unique(name, quad.triple),
        }
    }

    /// True when the dataset contains an equal-by-canonical-bytes quad.
    pub fn contains_quad(&self, quad: &Quad) -> bool {
        match &quad.graph_name {
            None => {
                let key = quad.triple.canonical_bytes();
                self.default_graph
                    .iter()
                    .any(|t| t.canonical_bytes() == key)
            }
            Some(name) => self
                .named_graph(name)
                .is_some_and(|g| g.contains(&quad.triple)),
        }
    }

    /// Remove an equal-by-canonical-bytes quad; returns `true` when the dataset
    /// changed. An emptied named graph is dropped from `named_graphs`.
    pub fn remove_quad(&mut self, quad: &Quad) -> bool {
        match &quad.graph_name {
            None => {
                let key = quad.triple.canonical_bytes();
                let idx = self
                    .default_graph
                    .iter()
                    .position(|t| t.canonical_bytes() == key);
                match idx {
                    Some(i) => {
                        self.default_graph.remove(i);
                        true
                    }
                    None => false,
                }
            }
            Some(name) => {
                let gidx = match self.named_graphs.iter().position(|g| &g.name == name) {
                    Some(i) => i,
                    None => return false,
                };
                if self.named_graphs[gidx].remove(&quad.triple).is_none() {
                    return false;
                }
                if self.named_graphs[gidx].is_empty() {
                    self.named_graphs.remove(gidx);
                }
                true
            }
        }
    }

    /// Deduplicate every graph (default + named). Returns the total number of
    /// duplicate triples removed.
    pub fn dedup(&mut self) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        let before = self.default_graph.len();
        self.default_graph
            .retain(|t| seen.insert(t.canonical_bytes()));
        let mut removed = before - self.default_graph.len();
        for graph in &mut self.named_graphs {
            removed += graph.dedup();
        }
        removed
    }

    /// Set-union merge: inserts `other`'s quads with set semantics, never
    /// growing duplicates. Returns the number of triples actually added.
    pub fn merge_set(&mut self, other: Dataset) -> usize {
        let mut added = 0;
        for quad in other.quads() {
            if self.insert_quad_unique(quad) {
                added += 1;
            }
        }
        added
    }

    pub fn named_graph(&self, name: &Iri) -> Option<&NamedGraph> {
        self.named_graphs.iter().find(|g| &g.name == name)
    }

    pub fn named_graph_mut(&mut self, name: &Iri) -> Option<&mut NamedGraph> {
        self.named_graphs.iter_mut().find(|g| &g.name == name)
    }

    pub fn graph_count(&self) -> usize {
        1 + self.named_graphs.len()
    }

    pub fn triple_count(&self) -> usize {
        self.default_graph.len() + self.named_graphs.iter().map(NamedGraph::len).sum::<usize>()
    }

    pub fn quads(&self) -> Vec<Quad> {
        let mut out = Vec::with_capacity(self.triple_count());
        for triple in &self.default_graph {
            out.push(Quad::in_default_graph(triple.clone()));
        }
        for graph in &self.named_graphs {
            out.extend(graph.to_quads());
        }
        out
    }

    pub fn default_statistics(&self) -> GraphStatistics {
        compute_triple_statistics(&self.default_graph)
    }

    /// Bridge to core [`DatasetObject`] (headers + stats, no triple payload).
    pub fn to_dataset_object(
        &self,
        object_id: ObjectId,
        created_at: TimestampMs,
    ) -> Result<DatasetObject, OntolithError> {
        let mut dataset = DatasetObject::new(object_id, created_at)?;
        dataset.default_graph.statistics = self.default_statistics();

        for graph in &self.named_graphs {
            let gid = ObjectId::new(format!(
                "{}/graph/{}",
                dataset.header.id.as_str(),
                graph.name.as_str()
            ))
            .map_err(OntolithError::InvalidArgument)?;
            let mut go = graph.to_graph_object(gid, created_at);
            go.metadata = ObjectMetadata::default();
            dataset.add_named_graph(go)?;
        }
        Ok(dataset)
    }

    /// Merge another dataset into self (appends triples; does not deduplicate).
    pub fn merge(&mut self, other: Dataset) {
        self.default_graph.extend(other.default_graph);
        for graph in other.named_graphs {
            for triple in graph.triples {
                self.insert_named(graph.name.clone(), triple);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.default_graph.is_empty() && self.named_graphs.iter().all(NamedGraph::is_empty)
    }
}

impl CanonicalEncode for Dataset {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"DS");
        // default graph
        out.write_tag(b"GD");
        let mut default_encoded: Vec<Vec<u8>> = self
            .default_graph
            .iter()
            .map(CanonicalEncode::canonical_bytes)
            .collect();
        default_encoded.sort();
        out.write_u64(default_encoded.len() as u64);
        for bytes in default_encoded {
            out.write_bytes(&bytes);
        }

        // named graphs sorted by name
        let mut graphs = self.named_graphs.clone();
        graphs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        out.write_u64(graphs.len() as u64);
        for graph in graphs {
            graph.write_canonical(out);
        }
    }
}

fn compute_triple_statistics(triples: &[Triple]) -> GraphStatistics {
    use std::collections::BTreeSet;

    let mut subjects = BTreeSet::new();
    let mut predicates = BTreeSet::new();
    let mut objects = BTreeSet::new();

    for triple in triples {
        subjects.insert(triple.subject.get());
        predicates.insert(triple.predicate.as_str().to_owned());
        objects.insert(triple.object.canonical_bytes());
    }

    GraphStatistics {
        triple_count: triples.len() as u64,
        distinct_subjects: subjects.len() as u64,
        distinct_predicates: predicates.len() as u64,
        distinct_objects: objects.len() as u64,
    }
}

/// Helper: graph id for default/named used by higher layers.
pub fn graph_id_from_option(name: Option<&Iri>) -> GraphId {
    match name {
        Some(iri) => GraphId::Named(iri.clone()),
        None => GraphId::Default,
    }
}
