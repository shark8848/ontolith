//! Application service composing provider + index (P8-01/P8-02 interface base).

use std::sync::Arc;

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;

use crate::domain::{Embedding, EmbeddingProvider, SemanticHit, SemanticIndex};
use crate::infrastructure::InMemorySemanticIndex;

/// Default auto-index cap: bounds the semantic index size (P8-01 M3).
pub const DEFAULT_SEMANTIC_INDEX_CAP: usize = 100_000;

pub mod agent;

/// Semantic search service (P8-02 interface foundation): embed + index +
/// top-k retrieval in one handle.
///
/// M3: the index is boxed so the same service surface runs over either the
/// in-memory index or the RocksDB-persisted one; the index cap lives here so
/// every ingestion path (startup auto-index, explicit `POST /semantic/index`,
/// incremental write reconciliation) shares the same bound.
pub struct SemanticSearchService {
    provider: Arc<dyn EmbeddingProvider>,
    index: Box<dyn SemanticIndex>,
    cap: usize,
}

impl SemanticSearchService {
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self::with_cap(provider, DEFAULT_SEMANTIC_INDEX_CAP)
    }

    pub fn with_cap(provider: Arc<dyn EmbeddingProvider>, cap: usize) -> Self {
        let index = InMemorySemanticIndex::new(Arc::clone(&provider));
        Self {
            provider,
            index: Box::new(index),
            cap,
        }
    }

    /// Persistent variant (P8-01 M3): index lives in the dedicated `semantic`
    /// RocksDB column family and survives restarts.
    #[cfg(feature = "rocksdb-backend")]
    pub fn new_persistent(
        engine: Arc<ontolith_storage::infrastructure::RocksDbStorageEngine>,
        provider: Arc<dyn EmbeddingProvider>,
        cap: usize,
    ) -> Result<Self, OntolithError> {
        let index = crate::infrastructure::RocksSemanticIndex::new(engine, provider.clone())?;
        Ok(Self {
            provider,
            index: Box::new(index),
            cap,
        })
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

    /// Index a term (idempotent: duplicates are ignored). No-op beyond the
    /// configured index cap.
    pub fn index(&mut self, term: &Term) -> Result<(), OntolithError> {
        if self.index.len() >= self.cap {
            return Ok(());
        }
        self.index.upsert(term)
    }

    /// Batch-index terms (idempotent). Returns the number newly indexed;
    /// respects the configured cap.
    pub fn index_terms(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        if self.index.len() >= self.cap || terms.is_empty() {
            return Ok(0);
        }
        let remaining = self.cap.saturating_sub(self.index.len());
        let terms: Vec<Term> = terms
            .iter()
            .filter(|t| !self.index.contains(t))
            .take(remaining)
            .cloned()
            .collect();
        self.index.upsert_many(&terms)
    }

    /// Remove a term (idempotent: absent terms are ignored).
    pub fn remove(&mut self, term: &Term) -> Result<(), OntolithError> {
        self.index.remove(term)
    }

    /// Batch-remove terms (P8-01 M3 delete flow-back). Returns the number
    /// actually removed.
    pub fn remove_terms(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        self.index.remove_many(terms)
    }

    /// Whether a term is currently indexed.
    pub fn contains(&self, term: &Term) -> bool {
        self.index.contains(term)
    }

    /// All currently indexed terms (store-diff reconciliation).
    pub fn all_terms(&self) -> Vec<Term> {
        self.index.all_terms()
    }

    pub fn indexed_terms(&self) -> usize {
        self.index.len()
    }

    /// Semantic retrieval: query text -> top-k nearest indexed terms.
    /// `k` is clamped into `[1, MAX_TOP_K]`.
    pub fn search_text(&self, text: &str, k: usize) -> Result<Vec<SemanticHit>, OntolithError> {
        let query = self.provider.embed_text(text)?;
        self.search_embedding(&query, k)
    }

    /// Semantic retrieval from an already-embedded query (P8-02 latency KPI
    /// isolates index cost from query embedding).
    pub fn search_embedding(
        &self,
        query: &Embedding,
        k: usize,
    ) -> Result<Vec<SemanticHit>, OntolithError> {
        self.index.search(query, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MAX_TOP_K;
    use crate::infrastructure::FeatureHashEmbedding;

    fn provider() -> Arc<dyn EmbeddingProvider> {
        Arc::new(FeatureHashEmbedding::default())
    }

    #[test]
    fn service_roundtrip_and_defaults() {
        let mut svc = SemanticSearchService::new(provider());
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

    #[test]
    fn service_remove_and_contains() {
        let mut svc = SemanticSearchService::new(provider());
        let t = Term::iri("urn:ex:gone");
        svc.index(&t).unwrap();
        assert!(svc.contains(&t));
        svc.remove(&t).unwrap();
        assert!(!svc.contains(&t));
        assert_eq!(svc.indexed_terms(), 0);
    }

    #[test]
    fn service_cap_limits_index_growth() {
        let mut svc = SemanticSearchService::with_cap(provider(), 2);
        svc.index(&Term::iri("urn:ex:a")).unwrap();
        svc.index(&Term::iri("urn:ex:b")).unwrap();
        svc.index(&Term::iri("urn:ex:c")).unwrap();
        assert_eq!(svc.indexed_terms(), 2);
        let added = svc
            .index_terms(&[Term::iri("urn:ex:d"), Term::iri("urn:ex:e")])
            .unwrap();
        assert_eq!(added, 0);
    }
}
