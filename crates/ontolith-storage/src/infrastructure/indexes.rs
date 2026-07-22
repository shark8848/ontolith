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
}

impl GraphIndex {
    pub fn insert(&mut self, quad: &Quad) -> bool {
        let key = quad_key(quad);
        if !self.keys.insert(key) {
            return false;
        }
        self.all.push(quad.clone());
        if let Some(g) = &quad.graph_name {
            self.by_graph
                .entry(g.as_str().to_owned())
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

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
