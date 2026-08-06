//! Secondary index maintenance for the in-memory storage engine.
//!
//! Maintains all six triple permutations plus a named-graph map.
//! Updates are incremental (insert/remove) rather than full rebuild.

use ontolith_core::domain::{CanonicalEncode, Iri, NodeId};
use ontolith_rdf::domain::{Quad, Term, Triple};
use std::collections::{HashMap, HashSet};

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

fn object_key(object: &Term) -> Vec<u8> {
    object.canonical_bytes()
}

#[derive(Default)]
pub struct TripleIndexes {
    /// Set of committed triple keys for O(1) dedup / exact delete.
    pub keys: HashSet<Vec<u8>>,
    pub spo: HashMap<NodeId, Vec<Triple>>,
    pub sop: HashMap<NodeId, Vec<Triple>>,
    pub pso: HashMap<String, Vec<Triple>>,
    pub pos: HashMap<String, Vec<Triple>>,
    pub osp: HashMap<Vec<u8>, Vec<Triple>>,
    pub ops: HashMap<Vec<u8>, Vec<Triple>>,
}

impl TripleIndexes {
    pub fn insert(&mut self, triple: &Triple) -> bool {
        let key = triple_key(triple);
        if !self.keys.insert(key) {
            return false; // duplicate
        }
        self.spo
            .entry(triple.subject)
            .or_default()
            .push(triple.clone());
        self.sop
            .entry(triple.subject)
            .or_default()
            .push(triple.clone());
        self.pso
            .entry(triple.predicate.as_str().to_owned())
            .or_default()
            .push(triple.clone());
        self.pos
            .entry(triple.predicate.as_str().to_owned())
            .or_default()
            .push(triple.clone());
        let ok = object_key(&triple.object);
        self.osp.entry(ok.clone()).or_default().push(triple.clone());
        self.ops.entry(ok).or_default().push(triple.clone());
        true
    }

    pub fn remove_exact(&mut self, triple: &Triple) -> bool {
        let key = triple_key(triple);
        if !self.keys.remove(&key) {
            return false;
        }
        remove_from_list(self.spo.get_mut(&triple.subject), triple);
        remove_from_list(self.sop.get_mut(&triple.subject), triple);
        remove_from_list(self.pso.get_mut(triple.predicate.as_str()), triple);
        remove_from_list(self.pos.get_mut(triple.predicate.as_str()), triple);
        let ok = object_key(&triple.object);
        remove_from_list(self.osp.get_mut(&ok), triple);
        remove_from_list(self.ops.get_mut(&ok), triple);
        true
    }

    pub fn remove_by_subject(&mut self, subject: NodeId) -> Vec<Triple> {
        let Some(list) = self.spo.remove(&subject) else {
            return Vec::new();
        };
        self.sop.remove(&subject);
        for t in &list {
            let key = triple_key(t);
            self.keys.remove(&key);
            remove_from_list(self.pso.get_mut(t.predicate.as_str()), t);
            remove_from_list(self.pos.get_mut(t.predicate.as_str()), t);
            let ok = object_key(&t.object);
            remove_from_list(self.osp.get_mut(&ok), t);
            remove_from_list(self.ops.get_mut(&ok), t);
        }
        list
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn by_subject(&self, subject: NodeId) -> Vec<Triple> {
        self.spo.get(&subject).cloned().unwrap_or_default()
    }

    pub fn by_predicate(&self, predicate: &Iri) -> Vec<Triple> {
        self.pos
            .get(predicate.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn by_object(&self, object: &Term) -> Vec<Triple> {
        self.osp
            .get(&object_key(object))
            .cloned()
            .unwrap_or_default()
    }

    pub fn distinct_counts(&self) -> (u64, u64, u64) {
        (
            self.spo.len() as u64,
            self.pos.len() as u64,
            self.osp.len() as u64,
        )
    }
}

fn remove_from_list(list: Option<&mut Vec<Triple>>, triple: &Triple) {
    if let Some(v) = list {
        v.retain(|t| t != triple);
    }
}

#[derive(Default)]
pub struct GraphIndex {
    pub keys: HashSet<Vec<u8>>,
    /// graph IRI string → quads (default graph not stored here)
    pub by_graph: HashMap<String, Vec<Quad>>,
    pub all: Vec<Quad>,
    /// Named-graph position indexes (six-permutation equivalent for quads).
    /// Default-graph triples are covered by [`TripleIndexes`]; these maps hold
    /// only quads carrying an explicit graph name.
    pub by_subject: HashMap<NodeId, Vec<Quad>>,
    pub by_predicate: HashMap<String, Vec<Quad>>,
    pub by_object: HashMap<Vec<u8>, Vec<Quad>>,
}

impl GraphIndex {
    pub fn insert(&mut self, quad: &Quad) -> bool {
        let key = quad_key(quad);
        if !self.keys.insert(key) {
            return false;
        }
        self.all.push(quad.clone());
        if let Some(g) = &quad.graph_name {
            let graph = g.as_str().to_owned();
            self.by_graph.entry(graph).or_default().push(quad.clone());
            self.by_subject
                .entry(quad.triple.subject)
                .or_default()
                .push(quad.clone());
            self.by_predicate
                .entry(quad.triple.predicate.as_str().to_owned())
                .or_default()
                .push(quad.clone());
            self.by_object
                .entry(object_key(&quad.triple.object))
                .or_default()
                .push(quad.clone());
        }
        true
    }

    pub fn remove_exact(&mut self, quad: &Quad) -> bool {
        let key = quad_key(quad);
        if !self.keys.remove(&key) {
            return false;
        }
        self.all.retain(|q| q != quad);
        if let Some(g) = &quad.graph_name
            && let Some(list) = self.by_graph.get_mut(g.as_str())
        {
            list.retain(|q| q != quad);
        }
        remove_quad_from_position(self.by_subject.get_mut(&quad.triple.subject), quad);
        remove_quad_from_position(
            self.by_predicate.get_mut(quad.triple.predicate.as_str()),
            quad,
        );
        remove_quad_from_position(
            self.by_object.get_mut(&object_key(&quad.triple.object)),
            quad,
        );
        true
    }

    pub fn remove_by_subject(&mut self, subject: NodeId) -> usize {
        let before = self.all.len();
        let removed: Vec<Quad> = self
            .all
            .iter()
            .filter(|q| q.triple.subject == subject)
            .cloned()
            .collect();
        for q in &removed {
            let key = quad_key(q);
            self.keys.remove(&key);
            if let Some(g) = &q.graph_name
                && let Some(list) = self.by_graph.get_mut(g.as_str())
            {
                list.retain(|x| x != q);
            }
            remove_quad_from_position(self.by_subject.get_mut(&q.triple.subject), q);
            remove_quad_from_position(self.by_predicate.get_mut(q.triple.predicate.as_str()), q);
            remove_quad_from_position(self.by_object.get_mut(&object_key(&q.triple.object)), q);
        }
        self.all.retain(|q| q.triple.subject != subject);
        before - self.all.len()
    }

    pub fn by_graph_name(&self, name: &Iri) -> Vec<Quad> {
        self.by_graph
            .get(name.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn by_subject_in_named_graphs(&self, subject: NodeId) -> Vec<Quad> {
        self.by_subject.get(&subject).cloned().unwrap_or_default()
    }

    pub fn by_predicate_in_named_graphs(&self, predicate: &Iri) -> Vec<Quad> {
        self.by_predicate
            .get(predicate.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn by_object_in_named_graphs(&self, object: &Term) -> Vec<Quad> {
        self.by_object
            .get(&object_key(object))
            .cloned()
            .unwrap_or_default()
    }

    /// Pick the most selective bound position over named-graph indexes, then
    /// filter by the remaining bound positions and the target graph.
    pub fn matching_in_named_graphs(
        &self,
        graph: Option<&Iri>,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
    ) -> Vec<Quad> {
        let mut quads = if let Some(s) = subject {
            self.by_subject_in_named_graphs(s)
        } else if let Some(p) = predicate {
            self.by_predicate_in_named_graphs(p)
        } else if let Some(o) = object {
            self.by_object_in_named_graphs(o)
        } else {
            self.all
                .iter()
                .filter(|q| q.graph_name.is_some())
                .cloned()
                .collect()
        };
        if let Some(p) = predicate {
            quads.retain(|q| &q.triple.predicate == p);
        }
        if let Some(o) = object {
            quads.retain(|q| &q.triple.object == o);
        }
        if let Some(s) = subject {
            quads.retain(|q| q.triple.subject == s);
        }
        if let Some(g) = graph {
            quads.retain(|q| q.graph_name.as_ref() == Some(g));
        }
        quads
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

fn remove_quad_from_position(list: Option<&mut Vec<Quad>>, quad: &Quad) {
    if let Some(v) = list {
        v.retain(|q| q != quad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_core::domain::{Iri, NodeId};

    fn named_quad(s: u64, p: &str, o: &str, g: &str) -> Quad {
        Quad::in_named_graph(
            Triple::new(NodeId::new(s), Iri::new(p), Term::Iri(Iri::new(o))),
            Iri::new(g),
        )
    }

    fn default_quad(s: u64, p: &str, o: &str) -> Quad {
        Quad::in_default_graph(Triple::new(
            NodeId::new(s),
            Iri::new(p),
            Term::Iri(Iri::new(o)),
        ))
    }

    #[test]
    fn named_graph_six_permutation_indexes_are_maintained() {
        let mut idx = GraphIndex::default();
        let q1 = named_quad(1, "urn:p", "urn:o1", "urn:g");
        let q2 = named_quad(1, "urn:q", "urn:o2", "urn:g");
        let q3 = named_quad(2, "urn:p", "urn:o1", "urn:g2");
        assert!(idx.insert(&q1));
        assert!(idx.insert(&q2));
        assert!(idx.insert(&q3));
        assert!(!idx.insert(&q1)); // duplicate no-op

        assert_eq!(idx.by_subject_in_named_graphs(NodeId::new(1)).len(), 2);
        assert_eq!(
            idx.by_predicate_in_named_graphs(&Iri::new("urn:p")).len(),
            2
        );
        assert_eq!(
            idx.by_object_in_named_graphs(&Term::Iri(Iri::new("urn:o1")))
                .len(),
            2
        );
        assert_eq!(idx.by_graph_name(&Iri::new("urn:g")).len(), 2);
    }

    #[test]
    fn default_graph_quads_are_not_position_indexed() {
        let mut idx = GraphIndex::default();
        idx.insert(&default_quad(1, "urn:p", "urn:o"));
        assert_eq!(idx.all.len(), 1);
        assert!(idx.by_subject_in_named_graphs(NodeId::new(1)).is_empty());
        assert!(idx.by_graph_name(&Iri::new("urn:g")).is_empty());
    }

    #[test]
    fn matching_in_named_graphs_filters_by_bound_positions() {
        let mut idx = GraphIndex::default();
        idx.insert(&named_quad(1, "urn:p", "urn:o1", "urn:g"));
        idx.insert(&named_quad(1, "urn:p", "urn:o2", "urn:g"));
        idx.insert(&named_quad(2, "urn:p", "urn:o2", "urn:g2"));

        let matched = idx.matching_in_named_graphs(
            Some(&Iri::new("urn:g")),
            Some(NodeId::new(1)),
            Some(&Iri::new("urn:p")),
            None,
        );
        assert_eq!(matched.len(), 2);

        let exact = idx.matching_in_named_graphs(
            Some(&Iri::new("urn:g2")),
            Some(NodeId::new(2)),
            Some(&Iri::new("urn:p")),
            Some(&Term::Iri(Iri::new("urn:o2"))),
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].triple.subject, NodeId::new(2));

        assert_eq!(
            idx.matching_in_named_graphs(Some(&Iri::new("urn:g")), None, None, None)
                .len(),
            2
        );
    }

    #[test]
    fn removal_updates_position_indexes() {
        let mut idx = GraphIndex::default();
        let q = named_quad(1, "urn:p", "urn:o", "urn:g");
        idx.insert(&q);
        assert!(idx.remove_exact(&q));
        assert!(idx.all.is_empty());
        assert!(idx.by_subject_in_named_graphs(NodeId::new(1)).is_empty());
        assert!(
            idx.by_predicate_in_named_graphs(&Iri::new("urn:p"))
                .is_empty()
        );

        idx.insert(&named_quad(1, "urn:p", "urn:o1", "urn:g"));
        idx.insert(&named_quad(1, "urn:q", "urn:o2", "urn:g"));
        assert_eq!(idx.remove_by_subject(NodeId::new(1)), 2);
        assert!(idx.all.is_empty());
        assert!(idx.by_graph_name(&Iri::new("urn:g")).is_empty());
    }
}
