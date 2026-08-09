//! P8-02 exit-criteria gate: semantic retrieval KPI guardrails (L8 / KPI §5).
//!
//! Determinism: the same query twice must yield byte-identical result
//! sequences (term + fixed-width score), matching the HTTP response contract.
//! Relevance: a controlled corpus must rank the related anchor term first.
//! Latency: top-10 retrieval over the 10k-term in-memory index stays under
//! the 1ms budget (query embedding cost excluded via `search_embedding`).
//! The latency assertion is a production-profile metric; CI runs this gate
//! with `cargo test --release`.

use ontolith_ai::application::SemanticSearchService;
use ontolith_ai::domain::{EmbeddingProvider, SemanticHit};
use ontolith_ai::infrastructure::FeatureHashEmbedding;
use ontolith_rdf::domain::Term;
use std::sync::Arc;
use std::time::Instant;

const CORPUS_SIZE: usize = 10_000;
/// P8-02 latency budget (ns): top-10 retrieval, 10k terms, 256 dims.
const BUDGET_NS: u128 = 1_000_000;

fn service() -> SemanticSearchService {
    let provider = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
    SemanticSearchService::new(provider)
}

fn canonical(hits: &[SemanticHit]) -> String {
    hits.iter()
        .map(|h| format!("{:?}|{:.6}", h.term, h.score))
        .collect::<Vec<_>>()
        .join("\n")
}

fn corpus(prefix: &str) -> Vec<Term> {
    (0..CORPUS_SIZE)
        .map(|i| Term::iri(format!("{prefix}:term_{i:05}_query")))
        .collect()
}

#[test]
fn retrieval_is_byte_deterministic() {
    let mut svc = service();
    svc.index_terms(&corpus("urn:ex")).unwrap();
    let q = svc.embed_text("sparql").unwrap();
    let first = canonical(&svc.search_embedding(&q, 10).unwrap());
    let second = canonical(&svc.search_embedding(&q, 10).unwrap());
    assert_eq!(
        first, second,
        "same query must return byte-identical results (term + score)"
    );
}

#[test]
fn retrieval_ranks_related_terms_first() {
    let mut svc = service();
    let mut terms = corpus("urn:ex");
    terms.extend([
        Term::iri("urn:ex:sparql_query"),
        Term::iri("urn:ex:email_address"),
        Term::iri("urn:ex:telephone_number"),
    ]);
    svc.index_terms(&terms).unwrap();

    for (query, expected) in [
        ("sparql", "urn:ex:sparql_query"),
        ("email", "urn:ex:email_address"),
        ("telephone", "urn:ex:telephone_number"),
    ] {
        let q = svc.embed_text(query).unwrap();
        let hits = svc.search_embedding(&q, 10).unwrap();
        assert!(!hits.is_empty(), "query {query:?} must return hits");
        assert_eq!(
            hits[0].term,
            Term::iri(expected),
            "query {query:?} must rank {expected:?} first (got {:?})",
            hits[0].term
        );
        assert!(hits[0].score >= hits[1].score, "scores must be descending");
    }
}

#[test]
fn retrieval_stays_within_latency_budget() {
    // The latency budget is a production-profile metric (release). Debug
    // builds run ~10-20x slower, so the assertion is enforced by the
    // `retrieval-gates` CI job (cargo test --release); here we only assert in
    // release to keep `cargo test --workspace` meaningful in both profiles.
    if cfg!(debug_assertions) {
        return;
    }
    let mut svc = service();
    svc.index_terms(&corpus("urn:ex")).unwrap();
    assert_eq!(svc.indexed_terms(), CORPUS_SIZE);
    let q = svc.embed_text("sparql").unwrap();

    // Warmup caches before measuring; median of samples dampens CI variance.
    for _ in 0..20 {
        let _ = svc.search_embedding(&q, 10).unwrap();
    }
    let mut samples = Vec::with_capacity(9);
    for _ in 0..9 {
        let start = Instant::now();
        let hits = svc.search_embedding(&q, 10).unwrap();
        assert_eq!(hits.len(), 10);
        samples.push(start.elapsed().as_nanos());
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    assert!(
        median < BUDGET_NS,
        "top-10 retrieval over {CORPUS_SIZE} terms took {median} ns (budget {BUDGET_NS} ns)"
    );
}
