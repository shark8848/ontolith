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

/// In-memory semantic index (P8-01 M1): linear `term -> embedding` store.
///
/// M1 deliberately avoids coupling to the storage layer; persistence lands in
/// M3 (RocksDB 独立 CF). Upsert dedups by term equality (linear scan).
pub struct InMemorySemanticIndex {
    provider: Arc<dyn EmbeddingProvider>,
    entries: Vec<(Term, Embedding)>,
}

impl InMemorySemanticIndex {
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            entries: Vec::new(),
        }
    }

    pub fn provider(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.provider
    }
}

impl SemanticIndex for InMemorySemanticIndex {
    fn upsert(&mut self, term: &Term) -> Result<(), OntolithError> {
        if self.entries.iter().any(|(t, _)| t == term) {
            return Ok(());
        }
        let embedding = self.provider.embed_term(term)?;
        self.entries.push((term.clone(), embedding));
        Ok(())
    }

    fn remove(&mut self, term: &Term) -> Result<(), OntolithError> {
        self.entries.retain(|(t, _)| t != term);
        Ok(())
    }

    fn contains(&self, term: &Term) -> bool {
        self.entries.iter().any(|(t, _)| t == term)
    }

    fn all_terms(&self) -> Vec<Term> {
        self.entries.iter().map(|(t, _)| t.clone()).collect()
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
        let mut scored: Vec<SemanticHit> = Vec::with_capacity(self.entries.len());
        for (term, emb) in &self.entries {
            let score = query.cosine_similarity(emb)?;
            scored.push(SemanticHit {
                term: term.clone(),
                score,
            });
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(k);
        Ok(scored)
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
