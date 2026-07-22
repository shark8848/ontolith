//! Triple / Quad statements (SAS-0401 §6, L1).
//!
//! Statements are immutable value types. They reference resources primarily via
//! dictionary [`NodeId`] (subject) and keep predicate IRIs in textual form for
//! the current storage path compatibility.

use ontolith_core::domain::{
    CanonicalEncode, CanonicalWriter, GraphId, Iri, KnowledgeObjectHeader, NodeId, ObjectId,
    ObjectType, TimestampMs,
};
use ontolith_core::error::OntolithError;

use super::term::Term;

/// RDF triple: subject / predicate / object.
///
/// Field layout is part of the storage/query contract — keep names stable.
#[derive(Debug, Clone, PartialEq)]
pub struct Triple {
    pub subject: NodeId,
    pub predicate: Iri,
    pub object: Term,
}

impl Triple {
    pub fn new(subject: NodeId, predicate: Iri, object: Term) -> Self {
        Self {
            subject,
            predicate,
            object,
        }
    }

    /// Validate structural constraints for R1:
    /// - predicate must be a non-empty absolute-ish IRI
    pub fn validated(self) -> Result<Self, OntolithError> {
        let _ = Iri::parse(self.predicate.as_str())?;
        Ok(self)
    }

    pub fn with_graph(self, graph_name: Option<Iri>) -> Quad {
        Quad {
            triple: self,
            graph_name,
        }
    }
}

impl CanonicalEncode for Triple {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"T3");
        out.write_u64(self.subject.get());
        out.write_str(self.predicate.as_str());
        self.object.write_canonical(out);
    }
}

/// RDF quad: triple + optional graph name (`None` = default graph).
#[derive(Debug, Clone, PartialEq)]
pub struct Quad {
    pub triple: Triple,
    pub graph_name: Option<Iri>,
}

impl Quad {
    pub fn new(triple: Triple, graph_name: Option<Iri>) -> Self {
        Self { triple, graph_name }
    }

    pub fn in_default_graph(triple: Triple) -> Self {
        Self {
            triple,
            graph_name: None,
        }
    }

    pub fn in_named_graph(triple: Triple, graph_name: Iri) -> Self {
        Self {
            triple,
            graph_name: Some(graph_name),
        }
    }

    pub fn graph_id(&self) -> GraphId {
        match &self.graph_name {
            Some(iri) => GraphId::Named(iri.clone()),
            None => GraphId::Default,
        }
    }

    pub fn validated(self) -> Result<Self, OntolithError> {
        let triple = self.triple.validated()?;
        if let Some(name) = &self.graph_name {
            let _ = Iri::parse(name.as_str())?;
        }
        Ok(Self {
            triple,
            graph_name: self.graph_name,
        })
    }
}

impl CanonicalEncode for Quad {
    fn write_canonical(&self, out: &mut CanonicalWriter) {
        out.write_tag(b"T4");
        match &self.graph_name {
            None => out.write_tag(b"GD"),
            Some(iri) => {
                out.write_tag(b"GN");
                out.write_str(iri.as_str());
            }
        }
        self.triple.write_canonical(out);
    }
}

/// Optional Knowledge Object wrapper for a statement (SAS-0401 Statement category).
///
/// Storage hot path continues to use bare [`Triple`] / [`Quad`]. This type is
/// for catalog / versioning / audit surfaces that need KO headers.
#[derive(Debug, Clone, PartialEq)]
pub struct StatementObject {
    pub header: KnowledgeObjectHeader,
    pub quad: Quad,
}

impl StatementObject {
    pub fn from_triple(
        object_id: ObjectId,
        triple: Triple,
        created_at: TimestampMs,
    ) -> Result<Self, OntolithError> {
        let quad = Quad::in_default_graph(triple).validated()?;
        Ok(Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Statement, created_at),
            quad,
        })
    }

    pub fn from_quad(
        object_id: ObjectId,
        quad: Quad,
        created_at: TimestampMs,
    ) -> Result<Self, OntolithError> {
        let quad = quad.validated()?;
        Ok(Self {
            header: KnowledgeObjectHeader::new(object_id, ObjectType::Statement, created_at),
            quad,
        })
    }
}
