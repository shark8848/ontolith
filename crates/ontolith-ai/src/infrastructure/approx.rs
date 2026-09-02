//! Approximate nearest-neighbor semantic index (P8-01 ANN, ADR-0006):
//! deterministic multi-probe hyperplane LSH over L2-normalized embeddings.
//!
//! Exact dot-product scores are computed only over candidate buckets, so
//! top-k over large indexes scans a fraction of the rows; recall is bounded
//! by the Hamming radius probed. Below `exact_below` entries the index scans
//! exactly, and whenever the candidate pool is too small it falls back to a
//! full scan — so results are never truncated, only possibly re-ranked. The
//! in-memory linear index remains the exact default; this is an opt-in
//! approximation for large corpora.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;

use crate::domain::{Embedding, EmbeddingProvider, MAX_TOP_K, SemanticHit, SemanticIndex};
use crate::infrastructure::dot_runtime;

/// LSH tuning for [`LshSemanticIndex`].
#[derive(Debug, Clone, Copy)]
pub struct LshConfig {
    /// Number of hyperplanes (hash bits). More bits = finer buckets;
    /// clamped into `[1, 32]`.
    pub bits: u32,
    /// Hamming radius probed around the query bucket (multi-probe recall
    /// knob); clamped to `min(bits, 4)`.
    pub probes: u32,
    /// Indexes at or below this entry count are scanned exactly.
    pub exact_below: usize,
    /// Deterministic seed for the hyperplane projection matrix.
    pub seed: u64,
}

impl Default for LshConfig {
    fn default() -> Self {
        Self {
            bits: 12,
            probes: 2,
            exact_below: 64,
            seed: 0x4f75_8d3c_9a2e_1b67,
        }
    }
}

/// SplitMix64: deterministic PRNG for the projection matrix (no external
/// RNG dependency; same seed -> same planes across processes/restarts).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Deterministic hyperplane-LSH semantic index (ANN, ADR-0006).
pub struct LshSemanticIndex {
    provider: Arc<dyn EmbeddingProvider>,
    entries: Vec<Term>,
    /// Flat row-major embedding matrix, same layout as the exact index.
    values: Vec<f32>,
    dim: usize,
    bits: u32,
    probes: u32,
    exact_below: usize,
    /// Hyperplane weights, `bits * dim` f32 values; plane `b` at
    /// `planes[b * dim .. (b + 1) * dim]`.
    planes: Vec<f32>,
    /// Bucket key (sign bits of the plane projections) -> entry indices.
    buckets: HashMap<u64, Vec<usize>>,
}

impl LshSemanticIndex {
    pub fn new(provider: Arc<dyn EmbeddingProvider>, config: LshConfig) -> Self {
        let dim = provider.dim();
        let bits = config.bits.clamp(1, 32);
        let probes = config.probes.min(bits).min(4);
        let mut seed = config.seed;
        let mut planes = Vec::with_capacity(bits as usize * dim);
        for _ in 0..(bits as usize * dim) {
            // 24-bit fraction in [0, 1) mapped to [-1, 1).
            let u = (splitmix64(&mut seed) >> 40) as f32 / (1u64 << 24) as f32;
            planes.push(u * 2.0 - 1.0);
        }
        Self {
            provider,
            entries: Vec::new(),
            values: Vec::new(),
            dim,
            bits,
            probes,
            exact_below: config.exact_below,
            planes,
            buckets: HashMap::new(),
        }
    }

    pub fn provider(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.provider
    }

    fn hash_row(&self, row: &[f32]) -> u64 {
        let mut key = 0u64;
        for b in 0..self.bits as usize {
            let plane = &self.planes[b * self.dim..(b + 1) * self.dim];
            let mut dot = 0.0f32;
            for (p, v) in plane.iter().zip(row) {
                dot += p * v;
            }
            if dot >= 0.0 {
                key |= 1 << b;
            }
        }
        key
    }

    /// Query bucket plus all buckets within the configured Hamming radius.
    fn probe_keys(&self, key: u64) -> Vec<u64> {
        let mask = (1u64 << self.bits) - 1;
        let base = key & mask;
        let mut keys = vec![base];
        if self.probes == 0 {
            return keys;
        }
        let positions: Vec<u32> = (0..self.bits).collect();
        let mut combo: Vec<u32> = Vec::new();
        let mut emit = |combo: &[u32]| {
            let mut k = base;
            for &p in combo {
                k ^= 1 << p;
            }
            keys.push(k & mask);
        };
        fn combos(
            positions: &[u32],
            depth: usize,
            start: usize,
            combo: &mut Vec<u32>,
            emit: &mut impl FnMut(&[u32]),
        ) {
            if combo.len() == depth {
                emit(combo);
                return;
            }
            for i in start..positions.len() {
                combo.push(positions[i]);
                combos(positions, depth, i + 1, combo, emit);
                combo.pop();
            }
        }
        for depth in 1..=self.probes as usize {
            combos(&positions, depth, 0, &mut combo, &mut emit);
        }
        keys
    }

    fn rebuild_buckets(&mut self) {
        self.buckets.clear();
        for (i, row) in self.values.chunks_exact(self.dim).enumerate() {
            self.buckets.entry(self.hash_row(row)).or_default().push(i);
        }
    }
}

impl SemanticIndex for LshSemanticIndex {
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
        let idx = self.entries.len();
        self.entries.push(term.clone());
        self.values.extend_from_slice(&embedding.values);
        self.buckets
            .entry(self.hash_row(&embedding.values))
            .or_default()
            .push(idx);
        Ok(())
    }

    fn remove(&mut self, term: &Term) -> Result<(), OntolithError> {
        if let Some(idx) = self.entries.iter().position(|t| t == term) {
            self.entries.swap_remove(idx);
            let start = idx * self.dim;
            let len = self.values.len();
            self.values.copy_within((len - self.dim).., start);
            self.values.truncate(len - self.dim);
            self.rebuild_buckets();
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
        let k = k.clamp(1, MAX_TOP_K);
        if self.entries.is_empty() {
            return Ok(Vec::new());
        }
        if query.dim != self.dim {
            return Err(OntolithError::InvalidArgument(
                "embedding dimension mismatch in semantic index search",
            ));
        }
        let q = query.normalized()?;
        let qv = &q.values;
        // Candidate pool: LSH buckets within the probe radius; exact scan
        // below the small-index threshold or when the pool is too thin.
        let candidates: Option<Vec<usize>> = if self.entries.len() <= self.exact_below {
            None
        } else {
            let mut seen = HashSet::new();
            let mut cands = Vec::new();
            for key in self.probe_keys(self.hash_row(qv)) {
                if let Some(idx) = self.buckets.get(&key) {
                    for &i in idx {
                        if seen.insert(i) {
                            cands.push(i);
                        }
                    }
                }
            }
            if cands.len() < k { None } else { Some(cands) }
        };
        let mut scored: Vec<(f32, usize)> = match candidates {
            Some(cands) => cands
                .into_iter()
                .map(|i| {
                    let row = &self.values[i * self.dim..(i + 1) * self.dim];
                    (dot_runtime(qv, row), i)
                })
                .collect(),
            None => self
                .values
                .chunks_exact(self.dim)
                .enumerate()
                .map(|(i, row)| (dot_runtime(qv, row), i))
                .collect(),
        };
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
    use crate::infrastructure::{FeatureHashEmbedding, InMemorySemanticIndex};

    fn corpus() -> Vec<Term> {
        let heads = [
            "red apple",
            "green apple",
            "apple pie",
            "apple juice",
            "golden apple",
        ];
        let mut terms = Vec::new();
        for (h, head) in heads.iter().enumerate() {
            for i in 0..8 {
                terms.push(Term::iri(format!("urn:fruit:{h}:{i}:{head}")));
            }
        }
        let noise = [
            "quantum zorbax flux",
            "telephone booth operator",
            "ceramic turbine housing",
            "submarine navigation chart",
            "synthetic polymer lattice",
            "railway signal interlocking",
            "optical fiber inspection",
            "hydraulic valve calibration",
        ];
        for (h, head) in noise.iter().enumerate() {
            for i in 0..8 {
                terms.push(Term::iri(format!("urn:noise:{h}:{i}:{head}")));
            }
        }
        terms
    }

    fn exact_topk(terms: &[Term], query: &str, k: usize) -> Vec<Term> {
        let p = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut idx = InMemorySemanticIndex::new(p);
        idx.upsert_many(terms).unwrap();
        let q = idx.provider().embed_text(query).unwrap();
        idx.search(&q, k)
            .unwrap()
            .into_iter()
            .map(|h| h.term)
            .collect()
    }

    #[test]
    fn lsh_recall_at_1_matches_exact_on_corpus() {
        let terms = corpus();
        let p = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut idx = LshSemanticIndex::new(
            p,
            LshConfig {
                bits: 12,
                probes: 2,
                exact_below: 0,
                ..LshConfig::default()
            },
        );
        idx.upsert_many(&terms).unwrap();
        for query in [
            "apple",
            "apple pie",
            "red apple juice",
            "golden apple",
            "telephone booth",
        ] {
            let q = idx.provider().embed_text(query).unwrap();
            let approx: Vec<Term> = idx
                .search(&q, 5)
                .unwrap()
                .into_iter()
                .map(|h| h.term)
                .collect();
            let exact = exact_topk(&terms, query, 5);
            assert_eq!(approx.len(), 5, "query={query}");
            let group = if query.contains("apple") {
                "fruit"
            } else {
                "noise"
            };
            // ANN and exact agree on the top-5 sets (up to near-tie
            // reordering between identical-head variants).
            assert!(
                approx.contains(&exact[0]),
                "exact winner missing from ANN top-5 (query={query}): {approx:?}"
            );
            assert!(
                exact.contains(&approx[0]),
                "ANN winner missing from exact top-5 (query={query}): {exact:?}"
            );
            assert!(
                matches!(&approx[0], Term::Iri(iri) if iri.as_str().starts_with(&format!("urn:{group}:"))),
                "top hit must relate to the query group: {approx:?}"
            );
        }
    }

    #[test]
    fn lsh_is_deterministic() {
        let p = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut idx = LshSemanticIndex::new(p, LshConfig::default());
        idx.upsert_many(&corpus()).unwrap();
        let q = idx.provider().embed_text("apple juice").unwrap();
        let a = idx.search(&q, 5).unwrap();
        let b = idx.search(&q, 5).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn lsh_upsert_dedup_and_remove() {
        let p = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut idx = LshSemanticIndex::new(p, LshConfig::default());
        assert!(idx.is_empty());
        let t = Term::iri("urn:ex:dup");
        idx.upsert(&t).unwrap();
        idx.upsert(&t).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(idx.contains(&t));
        idx.remove(&t).unwrap();
        assert!(idx.is_empty());
        let q = idx.provider().embed_text("dup").unwrap();
        assert_eq!(idx.search(&q, 3).unwrap().len(), 0);
    }

    #[test]
    fn lsh_dimension_mismatch_is_an_error() {
        let p = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
        let mut idx = LshSemanticIndex::new(p, LshConfig::default());
        idx.upsert(&Term::iri("urn:ex:probe")).unwrap();
        let wrong = Embedding::new(vec![1.0f32; 4]).unwrap();
        let err = idx.search(&wrong, 3).unwrap_err();
        assert!(err.message().contains("dimension"), "err={err}");
    }
}
