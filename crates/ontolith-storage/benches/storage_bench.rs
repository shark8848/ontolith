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
use std::io::Write;
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
    // P7-02 trend record: append one JSON line per case when
    // ONTOLITH_BENCH_TREND_PATH is set (e.g. the CI bench job).
    if let Ok(path) = std::env::var("ONTOLITH_BENCH_TREND_PATH")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    {
        let run_id =
            std::env::var("ONTOLITH_BENCH_RUN_ID").unwrap_or_else(|_| chrono_ish_timestamp());
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

/// Compact UTC timestamp (`YYYYMMDDTHHMMSSZ`) without external dependencies.
fn chrono_ish_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch -> civil date (Howard Hinnant's algorithm).
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let (hh, mm, ss) = ((secs % 86_400) / 3_600, (secs % 3_600) / 60, secs % 60);
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
