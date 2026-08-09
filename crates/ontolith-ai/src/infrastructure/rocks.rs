//! RocksDB-persisted semantic index (P8-01 M3): `term -> embedding` in the
//! dedicated `semantic` column family of [`RocksDbStorageEngine`], accessed
//! only through the byte-level `semantic_cf_*` primitives. The vector store
//! stays isolated from the RDF data plane (R4 扩展安全与兼容门禁).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;
use ontolith_storage::infrastructure::{
    RocksDbStorageEngine, SemanticCfOp, decode_term, encode_term,
};

use crate::domain::{Embedding, EmbeddingProvider, SemanticHit, SemanticIndex};

/// Embedding wire format: `u32 BE dim` + `f32 LE values` (deterministic).
fn encode_embedding(embedding: &Embedding) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + embedding.values.len() * 4);
    buf.extend_from_slice(&(embedding.dim as u32).to_be_bytes());
    for v in &embedding.values {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

fn decode_embedding(bytes: &[u8]) -> Result<Embedding, OntolithError> {
    if bytes.len() < 4 {
        return Err(OntolithError::Storage("semantic embedding truncated"));
    }
    let dim = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != 4 + dim * 4 {
        return Err(OntolithError::Storage("semantic embedding length mismatch"));
    }
    let mut values = Vec::with_capacity(dim);
    for chunk in bytes[4..].chunks_exact(4) {
        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Embedding::new(values)
}

/// Durable semantic index over the dedicated `semantic` column family.
///
/// Writes go through the engine's durability path (fsync WAL by default);
/// reads are linear scans, mirroring the in-memory index for M3. The term
/// count is cached in memory and reconstructed from the CF at open.
pub struct RocksSemanticIndex {
    engine: Arc<RocksDbStorageEngine>,
    provider: Arc<dyn EmbeddingProvider>,
    count: AtomicUsize,
}

impl RocksSemanticIndex {
    pub fn new(
        engine: Arc<RocksDbStorageEngine>,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, OntolithError> {
        let count = engine.semantic_cf_scan_all()?.len();
        Ok(Self {
            engine,
            provider,
            count: AtomicUsize::new(count),
        })
    }

    pub fn provider(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.provider
    }
}

impl SemanticIndex for RocksSemanticIndex {
    fn upsert(&mut self, term: &Term) -> Result<(), OntolithError> {
        let key = encode_term(term);
        if self.engine.semantic_cf_get(&key)?.is_some() {
            return Ok(());
        }
        let embedding = self.provider.embed_term(term)?;
        self.engine
            .semantic_cf_put(&key, &encode_embedding(&embedding))?;
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn upsert_many(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        let mut ops = Vec::new();
        let mut added = 0usize;
        for term in terms {
            let key = encode_term(term);
            if self.engine.semantic_cf_get(&key)?.is_some() {
                continue;
            }
            let embedding = self.provider.embed_term(term)?;
            ops.push(SemanticCfOp::Put(key, encode_embedding(&embedding)));
            added += 1;
        }
        if !ops.is_empty() {
            self.engine.semantic_cf_write_batch(&ops)?;
            self.count.fetch_add(added, Ordering::SeqCst);
        }
        Ok(added)
    }

    fn remove(&mut self, term: &Term) -> Result<(), OntolithError> {
        let key = encode_term(term);
        if self.engine.semantic_cf_get(&key)?.is_none() {
            return Ok(());
        }
        self.engine.semantic_cf_delete(&key)?;
        self.count.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    fn remove_many(&mut self, terms: &[Term]) -> Result<usize, OntolithError> {
        let mut ops = Vec::new();
        let mut removed = 0usize;
        for term in terms {
            let key = encode_term(term);
            if self.engine.semantic_cf_get(&key)?.is_some() {
                ops.push(SemanticCfOp::Delete(key));
                removed += 1;
            }
        }
        if !ops.is_empty() {
            self.engine.semantic_cf_write_batch(&ops)?;
            self.count.fetch_sub(removed, Ordering::SeqCst);
        }
        Ok(removed)
    }

    fn contains(&self, term: &Term) -> bool {
        self.engine
            .semantic_cf_get(&encode_term(term))
            .map(|v| v.is_some())
            .unwrap_or(false)
    }

    fn all_terms(&self) -> Vec<Term> {
        self.engine
            .semantic_cf_scan_all()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(key, _)| {
                let mut off = 0usize;
                decode_term(&key, &mut off).ok()
            })
            .collect()
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn search(&self, query: &Embedding, k: usize) -> Result<Vec<SemanticHit>, OntolithError> {
        let k = k.clamp(1, crate::domain::MAX_TOP_K);
        let entries = self.engine.semantic_cf_scan_all()?;
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let mut off = 0usize;
            let term = decode_term(&key, &mut off)?;
            let embedding = decode_embedding(&value)?;
            let score = query.cosine_similarity(&embedding)?;
            scored.push(SemanticHit { term, score });
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(k);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::FeatureHashEmbedding;

    fn open_index(path: &std::path::Path) -> (RocksSemanticIndex, Arc<RocksDbStorageEngine>) {
        let engine = Arc::new(RocksDbStorageEngine::open(path).expect("open"));
        let provider = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let index = RocksSemanticIndex::new(Arc::clone(&engine), provider).expect("index");
        (index, engine)
    }

    #[test]
    fn rocks_index_roundtrip_remove_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let (mut index, _engine) = open_index(&dir.path().join("db"));
        let terms = [
            "urn:ex:sparql_query",
            "urn:ex:rdf_graph",
            "urn:ex:unrelated_thing",
        ]
        .map(Term::iri);
        assert_eq!(index.upsert_many(&terms).unwrap(), 3);
        assert_eq!(index.len(), 3);
        assert!(index.contains(&terms[0]));
        assert_eq!(index.upsert_many(&terms).unwrap(), 0, "idempotent");
        assert_eq!(index.remove_many(&terms[2..]).unwrap(), 1);
        assert_eq!(index.len(), 2);
        assert!(!index.contains(&terms[2]));
        let query = index.provider().embed_text("sparql").expect("query embed");
        let hits = index.search(&query, 10).unwrap();
        assert_eq!(hits[0].term, terms[0]);
    }

    #[test]
    fn rocks_index_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let (mut index, _engine) = open_index(&path);
            index
                .upsert_many(&[Term::iri("urn:ex:persist_me")])
                .unwrap();
            assert_eq!(index.len(), 1);
        }
        {
            let (index, _engine) = open_index(&path);
            assert_eq!(index.len(), 1, "count rebuilt from CF at open");
            assert!(index.contains(&Term::iri("urn:ex:persist_me")));
            let query = index.provider().embed_text("persist").expect("query embed");
            let hits = index.search(&query, 10).unwrap();
            assert_eq!(hits[0].term, Term::iri("urn:ex:persist_me"));
        }
    }

    #[test]
    fn rocks_index_batch_remove_is_durable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let (mut index, _engine) = open_index(&path);
            let terms = [Term::iri("urn:ex:a"), Term::iri("urn:ex:b")];
            index.upsert_many(&terms).unwrap();
            assert_eq!(index.remove_many(&terms).unwrap(), 2);
            assert!(index.is_empty());
        }
        {
            let (index, _engine) = open_index(&path);
            assert!(index.is_empty(), "removals survive reopen");
        }
    }
}
