//! Infrastructure: deterministic feature-hash embedding + in-memory index
//! and (P8-01 M3) the RocksDB-persisted semantic index.

use std::sync::Arc;

use ontolith_core::domain::LiteralValue;
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;

use crate::domain::{
    DEFAULT_EMBEDDING_DIM, Embedding, EmbeddingProvider, SemanticHit, SemanticIndex,
};

#[cfg(feature = "rocksdb-backend")]
mod rocks;

#[cfg(feature = "rocksdb-backend")]
pub use rocks::RocksSemanticIndex;

/// Deterministic feature-hash embedding (P8-01 fallback, zero external deps).
///
/// Tokenizes text into alphanumeric tokens plus character trigrams, hashes
/// each feature with FNV-1a 64 into the fixed-dimension vector (signed
/// accumulation), then L2-normalizes. Same input always yields the same
/// vector across processes and restarts.
pub struct FeatureHashEmbedding {
    dim: usize,
}

impl FeatureHashEmbedding {
    pub fn new(dim: usize) -> Result<Self, OntolithError> {
        if dim == 0 {
            return Err(OntolithError::InvalidArgument(
                "feature-hash embedding dimension must be non-zero",
            ));
        }
        Ok(Self { dim })
    }
}

impl Default for FeatureHashEmbedding {
    fn default() -> Self {
        Self {
            dim: DEFAULT_EMBEDDING_DIM,
        }
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Lowercase alphanumeric tokens (deterministic projection of arbitrary text).
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn char_trigrams(token: &str) -> Vec<String> {
    let bytes: Vec<char> = token.chars().collect();
    if bytes.len() < 3 {
        return vec![token.to_owned()];
    }
    bytes
        .windows(3)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

/// Feature projection: tokens + char trigrams, each prefixed to avoid
/// token/trigram collisions.
fn features(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in tokenize(text) {
        out.push(format!("t:{token}"));
        for tri in char_trigrams(&token) {
            out.push(format!("g:{tri}"));
        }
    }
    out
}

impl EmbeddingProvider for FeatureHashEmbedding {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_text(&self, text: &str) -> Result<Embedding, OntolithError> {
        let mut acc = vec![0.0f32; self.dim];
        let feats = features(text);
        if feats.is_empty() {
            // Empty/whitespace input: deterministic all-zero vector.
            return Embedding::new(acc);
        }
        for f in feats {
            let h = fnv1a64(f.as_bytes());
            let idx = (h % self.dim as u64) as usize;
            // Signed accumulation keeps hash collisions from concentrating.
            acc[idx] += if h & 1 == 1 { 1.0 } else { -1.0 };
        }
        Embedding::new(acc)?.normalized()
    }

    fn embed_term(&self, term: &Term) -> Result<Embedding, OntolithError> {
        let text = match term {
            Term::Iri(iri) => iri.as_str().to_owned(),
            Term::BlankNode(id) => format!("_:{}", id.get()),
            Term::Literal(lit) => literal_text(lit),
        };
        self.embed_text(&text)
    }
}

/// Deterministic text projection of a literal (datatype/lang participate so
/// `"1"^^xsd:boolean` and `true^^xsd:boolean` embed differently).
fn literal_text(lit: &LiteralValue) -> String {
    match lit {
        LiteralValue::Lang { value, lang } => {
            format!("{}@{lang}", value.to_lowercase())
        }
        LiteralValue::Typed { value, datatype } => {
            format!("{}^^{}", value.to_lowercase(), datatype.as_str())
        }
        LiteralValue::Boolean(b) => {
            if *b {
                "true^^http://www.w3.org/2001/XMLSchema#boolean".to_owned()
            } else {
                "false^^http://www.w3.org/2001/XMLSchema#boolean".to_owned()
            }
        }
        other => other.lexical_form().to_lowercase(),
    }
}

/// Dot product between a normalized query vector and one flat row of the
/// index matrix when the dimension is a compile-time constant. Borrowing the
/// rows as fixed-size arrays removes all per-element bounds checks and lets
/// LLVM fully unroll + vectorize the reduction (P8-02 latency KPI hot path).
#[inline]
fn dot_const<const D: usize>(qv: &[f32], row: &[f32]) -> f32 {
    let qa: &[f32; D] = qv[..D].try_into().expect("embedding dimension contract");
    let ra: &[f32; D] = row[..D].try_into().expect("embedding dimension contract");
    let mut acc0 = 0.0f32;
    let mut acc1 = 0.0f32;
    let mut acc2 = 0.0f32;
    let mut acc3 = 0.0f32;
    let mut i = 0usize;
    while i + 4 <= D {
        acc0 += qa[i] * ra[i];
        acc1 += qa[i + 1] * ra[i + 1];
        acc2 += qa[i + 2] * ra[i + 2];
        acc3 += qa[i + 3] * ra[i + 3];
        i += 4;
    }
    let mut tail = 0.0f32;
    while i < D {
        tail += qa[i] * ra[i];
        i += 1;
    }
    acc0 + acc1 + acc2 + acc3 + tail
}

/// Runtime-dimension fallback dot product for non-default embedding sizes.
/// Four-wide chunking keeps two independent chains of FMAs for ILP even when
/// LLVM cannot unroll with constant bounds.
#[inline]
fn dot_runtime(qv: &[f32], row: &[f32]) -> f32 {
    let mut acc = 0.0f32;
    for (a, b) in qv.chunks_exact(4).zip(row.chunks_exact(4)) {
        acc += a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    }
    let mut tail = 0.0f32;
    let rem = qv.len() % 4;
    for i in (qv.len() - rem)..qv.len() {
        tail += qv[i] * row[i];
    }
    acc + tail
}

/// In-memory semantic index (P8-01 M1): linear `term -> embedding` store.
///
/// M1 deliberately avoids coupling to the storage layer; persistence lands in
/// M3 (RocksDB 独立 CF). Embeddings live in one contiguous row-major matrix
/// (`values`), which keeps the top-k scan cache-friendly — the P8-02 latency
/// KPI hot path. Upsert dedups by term equality (linear scan).
pub struct InMemorySemanticIndex {
    provider: Arc<dyn EmbeddingProvider>,
    entries: Vec<Term>,
    /// Flat row-major embedding matrix: `entries.len() * dim` f32 values,
    /// row `i` at `values[i * dim .. (i + 1) * dim]`.
    values: Vec<f32>,
    dim: usize,
}

impl InMemorySemanticIndex {
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        let dim = provider.dim();
        Self {
            provider,
            entries: Vec::new(),
            values: Vec::new(),
            dim,
        }
    }

    pub fn provider(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.provider
    }
}

impl SemanticIndex for InMemorySemanticIndex {
    fn upsert(&mut self, term: &Term) -> Result<(), OntolithError> {
        if self.entries.iter().any(|t| t == term) {
            return Ok(());
        }
        let embedding = self.provider.embed_term(term)?;
        if embedding.dim != self.dim {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension mismatch in semantic index upsert",
            ));
        }
        self.entries.push(term.clone());
        self.values.extend_from_slice(&embedding.values);
        Ok(())
    }

    fn remove(&mut self, term: &Term) -> Result<(), OntolithError> {
        if let Some(idx) = self.entries.iter().position(|t| t == term) {
            self.entries.swap_remove(idx);
            let start = idx * self.dim;
            // Replace the removed row with the last row, then drop the tail.
            let len = self.values.len();
            self.values.copy_within((len - self.dim).., start);
            self.values.truncate(len - self.dim);
        }
        Ok(())
    }

    fn contains(&self, term: &Term) -> bool {
        self.entries.iter().any(|t| t == term)
    }

    fn all_terms(&self) -> Vec<Term> {
        self.entries.clone()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn search(&self, query: &Embedding, k: usize) -> Result<Vec<SemanticHit>, OntolithError> {
        let k = k.clamp(1, crate::domain::MAX_TOP_K);
        if self.entries.is_empty() {
            return Ok(Vec::new());
        }
        if query.dim != self.dim {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension mismatch in semantic index search",
            ));
        }
        // Stored embeddings are L2-normalized at embed time, so cosine
        // similarity reduces to one dot product per entry (P8-02 KPI).
        let q = query.normalized()?;
        // Score by index first: the top-k materialization clones only the
        // winning terms instead of every candidate (latency KPI hot path).
        let dim = self.dim;
        let values = &self.values[..];
        let qv = &q.values;
        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(values.len() / dim);
        if dim == crate::domain::DEFAULT_EMBEDDING_DIM {
            for (idx, row) in values.chunks_exact(dim).enumerate() {
                scored.push((
                    dot_const::<{ crate::domain::DEFAULT_EMBEDDING_DIM }>(qv, row),
                    idx,
                ));
            }
        } else {
            for (idx, row) in values.chunks_exact(dim).enumerate() {
                scored.push((dot_runtime(qv, row), idx));
            }
        }
        // Partial selection instead of a full sort: O(n) partition then a
        // tiny sort of the k winners (P8-02 latency KPI).
        let take = k.min(scored.len());
        scored.select_nth_unstable_by(take - 1, |a, b| b.0.total_cmp(&a.0));
        scored[..take].sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(take);
        Ok(scored
            .into_iter()
            .map(|(score, idx)| SemanticHit {
                term: self.entries[idx].clone(),
                score,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontolith_rdf::domain::Term;

    fn provider() -> FeatureHashEmbedding {
        FeatureHashEmbedding::default()
    }

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let p = provider();
        let a = p.embed_text("ontology engine").unwrap();
        let b = p.embed_text("ontology engine").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.dim, 256);
        let norm: f32 = a.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm={norm}");
    }

    #[test]
    fn related_terms_rank_above_unrelated() {
        let p = provider();
        let q = p.embed_text("email").unwrap();
        let related = p.embed_text("email address").unwrap();
        let unrelated = p.embed_text("telephone").unwrap();
        let s_rel = q.cosine_similarity(&related).unwrap();
        let s_unrel = q.cosine_similarity(&unrelated).unwrap();
        assert!(s_rel > s_unrel, "related={s_rel} unrelated={s_unrel}");
    }

    #[test]
    fn term_embeddings_are_distinct_and_stable() {
        let p = provider();
        let t1 = Term::iri("urn:ex:email_address");
        let t2 = Term::iri("urn:ex:telephone");
        let e1 = p.embed_term(&t1).unwrap();
        let e2 = p.embed_term(&t1).unwrap();
        let e3 = p.embed_term(&t2).unwrap();
        assert_eq!(e1, e2);
        assert_ne!(e1, e3);
    }

    #[test]
    fn index_topk_retrieval_orders_by_similarity() {
        let p = Arc::new(provider()) as Arc<dyn EmbeddingProvider>;
        let mut idx = InMemorySemanticIndex::new(p);
        for t in [
            "urn:ex:sparql_query",
            "urn:ex:rdf_graph",
            "urn:ex:unrelated_thing",
        ]
        .map(Term::iri)
        {
            idx.upsert(&t).unwrap();
        }
        let q = idx.provider().embed_text("sparql").unwrap();
        let hits = idx.search(&q, 10).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].term, Term::iri("urn:ex:sparql_query"));
        assert!(hits[0].score >= hits[1].score);
    }

    #[test]
    fn upsert_dedups_and_empty_index_returns_no_hits() {
        let p = Arc::new(provider()) as Arc<dyn EmbeddingProvider>;
        let mut idx = InMemorySemanticIndex::new(p);
        assert!(idx.is_empty());
        let t = Term::iri("urn:ex:dup");
        idx.upsert(&t).unwrap();
        idx.upsert(&t).unwrap();
        assert_eq!(idx.len(), 1);
        let q = idx.provider().embed_text("dup").unwrap();
        assert_eq!(idx.search(&q, 3).unwrap().len(), 1);
    }

    #[test]
    fn dimension_mismatch_is_an_error() {
        let p = Arc::new(provider()) as Arc<dyn EmbeddingProvider>;
        let mut idx = InMemorySemanticIndex::new(p);
        idx.upsert(&Term::iri("urn:ex:probe")).unwrap();
        let wrong = Embedding::new(vec![1.0f32; 4]).unwrap();
        let err = idx.search(&wrong, 3).unwrap_err();
        assert!(err.message().contains("dimension"), "err={err}");
    }

    #[test]
    fn topk_is_clamped() {
        let p = Arc::new(provider()) as Arc<dyn EmbeddingProvider>;
        let mut idx = InMemorySemanticIndex::new(p);
        for i in 0..20 {
            idx.upsert(&Term::iri(format!("urn:ex:t{i}"))).unwrap();
        }
        let q = idx.provider().embed_text("t0").unwrap();
        assert_eq!(idx.search(&q, 1000).unwrap().len(), 20);
        assert_eq!(idx.search(&q, 0).unwrap().len(), 1);
    }
}
