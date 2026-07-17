use ontolith_core::domain::{Iri, LiteralValue, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Iri(Iri),
    BlankNode(NodeId),
    Literal(LiteralValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Triple {
    pub subject: NodeId,
    pub predicate: Iri,
    pub object: Term,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub triple: Triple,
    pub graph_name: Option<Iri>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedGraph {
    pub name: Iri,
    pub triples: Vec<Triple>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dataset {
    pub default_graph: Vec<Triple>,
    pub named_graphs: Vec<NamedGraph>,
}

pub fn status() -> &'static str {
    "domain"
}
