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
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for (a, b) in self.values.iter().zip(other.values.iter()) {
            dot += a * b;
            na += a * a;
            nb += b * b;
        }
        let denom = (na * nb).sqrt();
        Ok(if denom == 0.0 { 0.0 } else { dot / denom })
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
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    /// Top-k nearest entries to `query` (cosine, descending). Dimension
    /// mismatch with stored vectors is an error; empty index yields no hits.
    fn search(&self, query: &Embedding, k: usize) -> Result<Vec<SemanticHit>, OntolithError>;
}
