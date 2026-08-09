//! Application service composing provider + index (P8-01/P8-02 interface base).

use std::sync::Arc;

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;

use crate::domain::{Embedding, EmbeddingProvider, SemanticHit, SemanticIndex};
use crate::infrastructure::InMemorySemanticIndex;

/// Semantic search service (P8-02 interface foundation): embed + index +
/// top-k retrieval in one handle.
pub struct SemanticSearchService {
    provider: Arc<dyn EmbeddingProvider>,
    index: InMemorySemanticIndex,
}

impl SemanticSearchService {
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        let index = InMemorySemanticIndex::new(Arc::clone(&provider));
        Self { provider, index }
    }

    pub fn provider_dim(&self) -> usize {
        self.provider.dim()
    }

    pub fn embed_text(&self, text: &str) -> Result<Embedding, OntolithError> {
        self.provider.embed_text(text)
    }

    pub fn embed_term(&self, term: &Term) -> Result<Embedding, OntolithError> {
        self.provider.embed_term(term)
    }

    /// Index a term (idempotent: duplicates are ignored).
    pub fn index(&mut self, term: &Term) -> Result<(), OntolithError> {
        self.index.upsert(term)
    }

    pub fn indexed_terms(&self) -> usize {
        self.index.len()
    }

    /// Semantic retrieval: query text -> top-k nearest indexed terms.
    /// `k` is clamped into `[1, MAX_TOP_K]`.
    pub fn search_text(&self, text: &str, k: usize) -> Result<Vec<SemanticHit>, OntolithError> {
        let query = self.provider.embed_text(text)?;
        self.index.search(&query, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MAX_TOP_K;
    use crate::infrastructure::FeatureHashEmbedding;

    #[test]
    fn service_roundtrip_and_defaults() {
        let provider = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut svc = SemanticSearchService::new(provider);
        assert_eq!(svc.provider_dim(), 256);
        for t in [
            "urn:ex:ontology",
            "urn:ex:knowledge_graph",
            "urn:ex:unrelated",
        ]
        .map(Term::iri)
        {
            svc.index(&t).unwrap();
        }
        assert_eq!(svc.indexed_terms(), 3);
        let hits = svc.search_text("ontology", MAX_TOP_K).unwrap();
        assert_eq!(hits[0].term, Term::iri("urn:ex:ontology"));
    }
}
