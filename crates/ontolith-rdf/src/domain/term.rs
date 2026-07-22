//! RDF terms and statement positions (L1).
//!
//! Storage/query currently bind subjects via dictionary [`NodeId`] and keep
//! predicates as [`Iri`] text. Objects use [`Term`], which may be an IRI,
//! a blank node id, or a literal.

use ontolith_core::domain::{
    BlankNodeId, CanonicalEncode, CanonicalWriter, Iri, Literal, LiteralValue, NodeId, Resource,
};
use ontolith_core::error::OntolithError;

/// RDF term appearing in object (and optionally subject) position.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Iri(Iri),
    BlankNode(NodeId),
    Literal(LiteralValue),
}

impl Term {
    pub fn iri(value: impl Into<String>) -> Self {
        Self::Iri(Iri::new(value))
    }

    pub fn blank(node_id: NodeId) -> Self {
        Self::BlankNode(node_id)
    }

    pub fn literal(value: LiteralValue) -> Self {
        Self::Literal(value)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Iri(_) => "iri",
            Self::BlankNode(_) => "blank_node",
            Self::Literal(_) => "literal",
        }
    }

    /// Convert to a core [`Resource`]. Blank nodes require a label; when only a
    /// `NodeId` is available we synthesize a stable label `n{id}`.
    pub fn to_resource(&self) -> Resource {
        match self {
            Self::Iri(iri) => Resource::Iri(iri.clone()),
            Self::BlankNode(id) => Resource::BlankNode(BlankNodeId::new(format!("n{}", id.get()))),
            Self::Literal(value) => Resource::Literal(Literal::new(value.clone())),
        }
    }
}

impl CanonicalEncode for Term {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        match self {
            Self::Iri(iri) => {
                out.write_tag(b"TI");
                out.write_str(iri.as_str());
            }
            Self::BlankNode(id) => {
                out.write_tag(b"TB");
                out.write_u64(id.get());
            }
            Self::Literal(value) => {
                out.write_tag(b"TL");
                value.write_canonical(out);
            }
        }
    }
}

impl From<Iri> for Term {
    fn from(value: Iri) -> Self {
        Self::Iri(value)
    }
}

impl From<LiteralValue> for Term {
    fn from(value: LiteralValue) -> Self {
        Self::Literal(value)
    }
}

/// Subject position: IRI-backed node id or blank node id (never literal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Subject(pub NodeId);

impl Subject {
    pub fn new(node_id: NodeId) -> Self {
        Self(node_id)
    }

    pub fn node_id(self) -> NodeId {
        self.0
    }
}

impl From<NodeId> for Subject {
    fn from(value: NodeId) -> Self {
        Self(value)
    }
}

impl CanonicalEncode for Subject {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"S");
        out.write_u64(self.0.get());
    }
}

/// Predicate position: always an IRI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Predicate(pub Iri);

impl Predicate {
    pub fn new(iri: Iri) -> Self {
        Self(iri)
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, OntolithError> {
        Ok(Self(Iri::parse(value)?))
    }

    pub fn as_iri(&self) -> &Iri {
        &self.0
    }
}

impl From<Iri> for Predicate {
    fn from(value: Iri) -> Self {
        Self(value)
    }
}

impl CanonicalEncode for Predicate {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"P");
        out.write_str(self.0.as_str());
    }
}
