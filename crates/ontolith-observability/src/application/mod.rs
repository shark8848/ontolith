use crate::domain::{MetricKind, MetricPoint};
use ontolith_core::domain::TimestampMs;
use ontolith_storage::infrastructure::{InMemoryStorageEngine, StorageMetricsSnapshot};
use ontolith_transaction::infrastructure::{
    InMemoryTransactionManager, TransactionMetricsSnapshot,
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMetricsSnapshot {
    pub timestamp_ms: TimestampMs,
    pub transaction: TransactionMetricsSnapshot,
    pub storage: StorageMetricsSnapshot,
}

pub trait TransactionMetricsReader {
    fn transaction_metrics_snapshot(&self) -> TransactionMetricsSnapshot;
}

pub trait StorageMetricsReader {
    fn storage_metrics_snapshot(&self) -> StorageMetricsSnapshot;
}

impl TransactionMetricsReader for InMemoryTransactionManager {
    fn transaction_metrics_snapshot(&self) -> TransactionMetricsSnapshot {
        self.metrics_snapshot()
    }
}

impl StorageMetricsReader for InMemoryStorageEngine {
    fn storage_metrics_snapshot(&self) -> StorageMetricsSnapshot {
        self.metrics_snapshot()
    }
}

pub fn collect_runtime_metrics<T, S>(
    tx_reader: &T,
    storage_reader: &S,
) -> RuntimeMetricsSnapshot
where
    T: TransactionMetricsReader,
    S: StorageMetricsReader,
{
    RuntimeMetricsSnapshot {
        timestamp_ms: now_ms(),
        transaction: tx_reader.transaction_metrics_snapshot(),
        storage: storage_reader.storage_metrics_snapshot(),
    }
}

pub fn runtime_snapshot_to_metric_points(snapshot: &RuntimeMetricsSnapshot) -> Vec<MetricPoint> {
    let timestamp_ms = snapshot.timestamp_ms;
    vec![
        counter(
            "transaction.begun",
            snapshot.transaction.begun,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        counter(
            "transaction.begin_rejected",
            snapshot.transaction.begin_rejected,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        counter(
            "transaction.committed",
            snapshot.transaction.committed,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        counter(
            "transaction.aborted",
            snapshot.transaction.aborted,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        counter(
            "transaction.expired_cleaned",
            snapshot.transaction.expired_cleaned,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        gauge(
            "transaction.active",
            snapshot.transaction.active as f64,
            timestamp_ms,
            &[ ("component", "transaction") ],
        ),
        counter(
            "storage.staged_batches",
            snapshot.storage.staged_batches,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.staged_batches,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "stage"),
                ("result", "success"),
            ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.failed_stage_batches,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "stage"),
                ("result", "failure"),
            ],
        ),
        counter(
            "storage.committed_transactions",
            snapshot.storage.committed_transactions,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.committed_transactions,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "commit"),
                ("result", "success"),
            ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.failed_commit_transactions,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "commit"),
                ("result", "failure"),
            ],
        ),
        counter(
            "storage.committed_write_operations",
            snapshot.storage.committed_put_triple_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "put_triple")],
        ),
        counter(
            "storage.committed_write_operations",
            snapshot.storage.committed_put_quad_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "put_quad")],
        ),
        counter(
            "storage.committed_write_operations",
            snapshot.storage.committed_delete_key_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "delete_key")],
        ),
        counter(
            "storage.aborted_write_operations",
            snapshot.storage.aborted_put_triple_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "put_triple")],
        ),
        counter(
            "storage.aborted_write_operations",
            snapshot.storage.aborted_put_quad_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "put_quad")],
        ),
        counter(
            "storage.aborted_write_operations",
            snapshot.storage.aborted_delete_key_operations,
            timestamp_ms,
            &[("component", "storage"), ("operation", "delete_key")],
        ),
        counter(
            "storage.aborted_transactions",
            snapshot.storage.aborted_transactions,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.aborted_transactions,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "abort"),
                ("result", "success"),
            ],
        ),
        counter(
            "storage.write_transactions",
            snapshot.storage.failed_abort_transactions,
            timestamp_ms,
            &[
                ("component", "storage"),
                ("phase", "abort"),
                ("result", "failure"),
            ],
        ),
        counter(
            "storage.checkpoint_truncated_records",
            snapshot.storage.checkpoint_truncated_records,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
        gauge(
            "storage.pending_transactions",
            snapshot.storage.pending_transactions as f64,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
        gauge(
            "storage.wal_records",
            snapshot.storage.wal_records as f64,
            timestamp_ms,
            &[ ("component", "storage") ],
        ),
    ]
}

fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as TimestampMs)
        .unwrap_or(0)
}

fn counter(
    name: &str,
    value: u64,
    timestamp_ms: TimestampMs,
    labels: &[(&str, &str)],
) -> MetricPoint {
    MetricPoint {
        name: name.to_owned(),
        labels: labels
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        kind: MetricKind::Counter,
        value: value as f64,
        timestamp_ms,
    }
}

fn gauge(
    name: &str,
    value: f64,
    timestamp_ms: TimestampMs,
    labels: &[(&str, &str)],
) -> MetricPoint {
    MetricPoint {
        name: name.to_owned(),
        labels: labels
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        kind: MetricKind::Gauge,
        value,
        timestamp_ms,
    }
}

pub fn status() -> &'static str {
    "application"
}

#[cfg(test)]
mod tests {
    use super::{collect_runtime_metrics, runtime_snapshot_to_metric_points};
    use ontolith_core::domain::{Iri, NodeId};
    use ontolith_rdf::domain::{Term, Triple};
    use ontolith_storage::application::StorageEngine;
    use ontolith_storage::domain::{WriteBatch, WriteOperation};
    use ontolith_storage::infrastructure::InMemoryStorageEngine;
    use ontolith_transaction::application::TransactionManager;
    use ontolith_transaction::domain::TxnMode;
    use ontolith_transaction::infrastructure::InMemoryTransactionManager;

    #[test]
    fn collect_runtime_metrics_reads_transaction_and_storage_counters() {
        let tx_manager = InMemoryTransactionManager::new();
        let storage = InMemoryStorageEngine::new();

        let txn = tx_manager
            .begin(TxnMode::ReadWrite)
            .expect("begin transaction");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: txn.id,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:metrics:predicate"),
                    object: Term::Iri(Iri::new("urn:test:metrics:value")),
                })],
            })
            .expect("stage write");
        storage
            .commit_transaction(txn.id)
            .expect("commit storage transaction");
        tx_manager
            .commit(txn.id)
            .expect("commit transaction manager state");

        let snapshot = collect_runtime_metrics(&tx_manager, &storage);

        assert_eq!(snapshot.transaction.begun, 1);
        assert_eq!(snapshot.transaction.committed, 1);
        assert_eq!(snapshot.transaction.active, 0);
        assert_eq!(snapshot.storage.staged_batches, 1);
        assert_eq!(snapshot.storage.committed_transactions, 1);
        assert_eq!(snapshot.storage.committed_put_triple_operations, 1);
        assert_eq!(snapshot.storage.committed_put_quad_operations, 0);
        assert_eq!(snapshot.storage.committed_delete_key_operations, 0);
        assert_eq!(snapshot.storage.failed_stage_batches, 0);
        assert_eq!(snapshot.storage.failed_commit_transactions, 0);
        assert_eq!(snapshot.storage.failed_abort_transactions, 0);
        assert_eq!(snapshot.storage.aborted_put_triple_operations, 0);
        assert_eq!(snapshot.storage.aborted_put_quad_operations, 0);
        assert_eq!(snapshot.storage.aborted_delete_key_operations, 0);
        assert_eq!(snapshot.storage.pending_transactions, 0);
        assert!(snapshot.timestamp_ms > 0);
    }

    #[test]
    fn metric_point_mapping_contains_runtime_core_points() {
        let tx_manager = InMemoryTransactionManager::new();
        let storage = InMemoryStorageEngine::new();

        let snapshot = collect_runtime_metrics(&tx_manager, &storage);
        let points = runtime_snapshot_to_metric_points(&snapshot);

        assert_eq!(points.len(), 24);
        assert!(points.iter().any(|point| point.name == "transaction.begun"));
        assert!(
            points
                .iter()
                .any(|point| point.name == "storage.committed_transactions")
        );
        assert_eq!(
            points
                .iter()
                .filter(|point| point.name == "storage.committed_write_operations")
                .count(),
            3
        );
        assert_eq!(
            points
                .iter()
                .filter(|point| point.name == "storage.aborted_write_operations")
                .count(),
            3
        );
        assert_eq!(
            points
                .iter()
                .filter(|point| point.name == "storage.write_transactions")
                .count(),
            6
        );
        assert!(points.iter().all(|point| !point.labels.is_empty()));
        assert!(
            points
                .iter()
                .filter(|point| point.name.starts_with("transaction."))
                .all(|point| point.labels.iter().any(|(k, v)| k == "component" && v == "transaction"))
        );
        assert!(points.iter().all(|point| point.timestamp_ms == snapshot.timestamp_ms));
    }
}
