//! Dependency-free semantic index micro-benchmarks (L8 / P8-02 KPI).
//!
//! Run: `cargo bench -p ontolith-ai --bench semantic_bench`
//!
//! Covers the retrieval hot paths behind the P8-02 KPI: top-k cosine search
//! over a 10k-term in-memory index (<1ms budget at 256 dims) and single-term
//! upsert.

use ontolith_ai::application::SemanticSearchService;
use ontolith_ai::domain::EmbeddingProvider;
use ontolith_ai::infrastructure::FeatureHashEmbedding;
use ontolith_rdf::domain::Term;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

const CORPUS_SIZE: usize = 10_000;

fn bench(name: &str, iterations: u64, mut f: impl FnMut(u64)) {
    for i in 0..iterations.min(1000) {
        f(i);
    }
    let started = Instant::now();
    for i in 0..iterations {
        f(i);
    }
    let elapsed = started.elapsed().as_nanos();
    let per_op = elapsed / iterations.max(1) as u128;
    println!(
        "{name:<32} {iterations:>10} ops  {per_op:>10} ns/op  total {:.3} ms",
        elapsed as f64 / 1_000_000.0
    );
    if let Ok(path) = std::env::var("ONTOLITH_BENCH_TREND_PATH")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    {
        let run_id = std::env::var("ONTOLITH_BENCH_RUN_ID").unwrap_or_else(|_| "local".to_owned());
        let _ = writeln!(
            file,
            r#"{{"run_id":{},"case":{},"iterations":{},"per_op_ns":{},"total_ms":{}}}"#,
            json_str(&run_id),
            json_str(name),
            iterations,
            per_op,
            elapsed as f64 / 1_000_000.0
        );
    }
}

fn json_str(s: &str) -> String {
    format!("\"{s}\"")
}

fn main() {
    let provider = Arc::new(FeatureHashEmbedding::default()) as Arc<dyn EmbeddingProvider>;
    let mut svc = SemanticSearchService::new(provider);
    let mut terms: Vec<Term> = (0..CORPUS_SIZE)
        .map(|i| Term::iri(format!("urn:ex:term_{i}_query")))
        .collect();
    // Anchor terms used by the search queries below.
    terms.push(Term::iri("urn:ex:sparql_query"));
    terms.push(Term::iri("urn:ex:email_address"));
    terms.push(Term::iri("urn:ex:telephone_number"));
    svc.index_terms(&terms).expect("index corpus");

    bench("semantic search top-10 (10k)", 1_000, |i| {
        let q = if i % 3 == 0 {
            "sparql"
        } else if i % 3 == 1 {
            "email"
        } else {
            "telephone"
        };
        let hits = svc.search_text(q, 10).expect("search");
        std::hint::black_box(&hits);
    });

    bench("semantic index upsert (1)", 1_000, |i| {
        let s = svc.index_terms(&[Term::iri(format!("urn:ex:hot_{i}"))]);
        std::hint::black_box(&s);
    });

    // Index-only retrieval: query embedded once, then top-k per iteration
    // (isolates the P8-02 latency KPI from query embedding cost).
    let q = svc.embed_text("sparql").expect("embed");
    bench("semantic search embed-only top-10 (10k)", 1_000, |_| {
        let hits = svc.search_embedding(&q, 10).expect("search");
        std::hint::black_box(&hits);
    });
}
