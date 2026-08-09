//! AI-native domain: embeddings, provider abstraction, semantic index (P8-01).

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;

/// Default embedding dimension (feature-hash fallback).
pub const DEFAULT_EMBEDDING_DIM: usize = 256;
/// Default top-k for semantic search.
pub const DEFAULT_TOP_K: usize = 10;
/// Hard cap for top-k (guards memory/CPU on adversarial callers).
pub const MAX_TOP_K: usize = 100;

/// Fixed-dimension, L2-normalized embedding vector.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub dim: usize,
    pub values: Vec<f32>,
}

impl Embedding {
    /// Validate dimension and finiteness (NaN/Inf explicitly rejected).
    pub fn new(values: Vec<f32>) -> Result<Self, OntolithError> {
        if values.is_empty() {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension must be non-zero",
            ));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(OntolithError::InvalidArgument(
                "embedding values must be finite",
            ));
        }
        let dim = values.len();
        Ok(Self { dim, values })
    }

    /// L2-normalized copy. Deterministic; a zero vector stays zero.
    pub fn normalized(&self) -> Result<Self, OntolithError> {
        let norm: f32 = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Ok(self.clone());
        }
        Self::new(self.values.iter().map(|v| v / norm).collect())
    }

    /// Cosine similarity in [-1, 1]; dimension mismatch is an error.
    pub fn cosine_similarity(&self, other: &Self) -> Result<f32, OntolithError> {
        if self.dim != other.dim {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension mismatch in cosine similarity",
            ));
        }
        let dot = self.dot(other)?;
        // L2 norms (self is typically normalized; computed defensively).
        let na = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        let nb = other.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        let denom = na * nb;
        Ok(if denom == 0.0 { 0.0 } else { dot / denom })
    }

    /// Dot product; dimension mismatch is an error. The hot path for
    /// top-k retrieval over pre-normalized embeddings (P8-02 latency KPI).
    #[inline]
    pub fn dot(&self, other: &Self) -> Result<f32, OntolithError> {
        if self.dim != other.dim {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension mismatch in dot product",
            ));
        }
        Ok(self.dot_values(&other.values))
    }

    /// Dot product against a raw vector slice (cache-friendly hot path for
    /// the flat in-memory index layout; no per-row allocation).
    #[inline]
    pub fn dot_values(&self, other: &[f32]) -> f32 {
        debug_assert_eq!(self.dim, other.len());
        let a = &self.values;
        let b = other;
        let mut acc0 = 0.0f32;
        let mut acc1 = 0.0f32;
        let mut acc2 = 0.0f32;
        let mut acc3 = 0.0f32;
        let mut i = 0usize;
        while i + 4 <= a.len() {
            acc0 += a[i] * b[i];
            acc1 += a[i + 1] * b[i + 1];
            acc2 += a[i + 2] * b[i + 2];
            acc3 += a[i + 3] * b[i + 3];
            i += 4;
        }
        let mut tail = 0.0f32;
        while i < a.len() {
            tail += a[i] * b[i];
            i += 1;
        }
        acc0 + acc1 + acc2 + acc3 + tail
    }
}

/// Embedding provider abstraction (P8-01): RDF terms / query text -> vectors.
pub trait EmbeddingProvider: Send + Sync {
    fn dim(&self) -> usize;
    fn embed_text(&self, text: &str) -> Result<Embedding, OntolithError>;
    fn embed_term(&self, term: &Term) -> Result<Embedding, OntolithError>;
}

/// One semantic hit: an indexed term plus its cosine score.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticHit {
    pub term: Term,
    pub score: f32,
}

/// Semantic term index (P8-01): upsert terms, top-k nearest by cosine.
pub trait SemanticIndex: Send + Sync {
    fn upsert(&mut self, term: &Term) -> Result<(), OntolithError>;

    /// Batch upsert (idempotent). Returns the number of terms that were not
    /// already present.
    fn upsert_many(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        let mut added = 0usize;
        for term in terms {
            if !self.contains(term) {
                self.upsert(term)?;
                added += 1;
            }
        }
        Ok(added)
    }

    /// Remove a term from the index (idempotent: absent terms are ignored).
    fn remove(&mut self, term: &Term) -> Result<(), OntolithError>;

    /// Batch remove. Returns the number of terms that were present.
    fn remove_many(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        let mut removed = 0usize;
        for term in terms {
            if self.contains(term) {
                self.remove(term)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Whether the index currently holds `term`.
    fn contains(&self, term: &Term) -> bool;

    /// All currently indexed terms (used for store-diff reconciliation).
    fn all_terms(&self) -> Vec<Term>;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    /// Top-k nearest entries to `query` (cosine, descending). Dimension
    /// mismatch with stored vectors is an error; empty index yields no hits.
    fn search(&self, query: &Embedding, k: usize) -> Result<Vec<SemanticHit>, OntolithError>;
}
