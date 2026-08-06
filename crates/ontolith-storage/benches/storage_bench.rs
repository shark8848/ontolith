//! Dependency-free storage micro-benchmarks (L7 performance baseline).
//!
//! Run: `cargo bench -p ontolith-storage`
//!
//! Covers the hot paths of the in-memory engine: dictionary encode,
//! transactional triple insert/commit, and multi-index matching.

use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::{DictionaryCodec, StorageEngine};
use ontolith_storage::domain::WriteOperation;
use ontolith_storage::infrastructure::{InMemoryDictionary, InMemoryStorageEngine};
use std::time::Instant;

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
        "{name:<28} {iterations:>10} ops  {per_op:>10} ns/op  total {:.3} ms",
        elapsed as f64 / 1_000_000.0
    );
}

fn main() {
    let iterations = 20_000u64;

    bench("dict encode_node", iterations, |i| {
        let dict = InMemoryDictionary::new();
        let _ = dict.encode_node(&format!("urn:resource:{i}"));
    });

    bench("triple insert + commit", iterations, |i| {
        let engine = InMemoryStorageEngine::new();
        let txn_id = ontolith_transaction::domain::TxnId::new(i as u128 + 1);
        let triple = Triple::new(
            NodeId::new(i),
            Iri::new("urn:predicate"),
            Term::Literal(LiteralValue::Integer(i as i64)),
        );
        engine
            .apply_write_batch(&ontolith_storage::domain::WriteBatch {
                txn_id,
                operations: vec![WriteOperation::PutTriple(triple)],
            })
            .expect("stage");
        engine.commit_transaction(txn_id).expect("commit");
    });

    bench("match by subject (1k triples)", 1_000, |i| {
        let engine = InMemoryStorageEngine::new();
        let txn = ontolith_transaction::domain::TxnId::new(i as u128 + 100_000);
        let mut ops = Vec::with_capacity(1_000);
        for k in 0..1_000u64 {
            ops.push(WriteOperation::PutTriple(Triple::new(
                NodeId::new(k),
                Iri::new("urn:p"),
                Term::Iri(Iri::new(format!("urn:o:{k}"))),
            )));
        }
        engine
            .apply_write_batch(&ontolith_storage::domain::WriteBatch {
                txn_id: txn,
                operations: ops,
            })
            .expect("stage");
        engine.commit_transaction(txn).expect("commit");
        let found = engine.triples_by_subject_in_txn(NodeId::new(500), None);
        assert_eq!(found.len(), 1);
    });

    println!("storage bench complete");
}
