use crate::{api, runtime};
use ontolith_observability::infrastructure::{
    run_runtime_sampling_loop, InMemoryMetricSink, RuntimeSamplingConfig,
};
use ontolith_storage::infrastructure::InMemoryStorageEngine;
use ontolith_transaction::infrastructure::InMemoryTransactionManager;
use std::env;

const METRICS_SAMPLE_ROUNDS_ENV: &str = "ONTOLITH_METRICS_SAMPLE_ROUNDS";
const METRICS_SAMPLE_INTERVAL_MS_ENV: &str = "ONTOLITH_METRICS_SAMPLE_INTERVAL_MS";

pub fn run() {
    let tx_manager = InMemoryTransactionManager::new();
    let storage = InMemoryStorageEngine::new();
    let sink = InMemoryMetricSink::new();
    let sampling_config = load_runtime_sampling_config_from_env();
    let snapshots = run_runtime_sampling_loop(
        &tx_manager,
        &storage,
        &sink,
        sampling_config,
    )
    .expect("runtime metrics sampling/export should succeed");
    let snapshot = snapshots
        .last()
        .expect("sampling loop must produce at least one snapshot");
    let exported_points = sink.points();
    let prometheus_text = api::metrics_text(&exported_points);
    let prometheus_line_count = prometheus_text.lines().count();

    println!(
        "ontolith-server bootstrap ready: api={}, runtime={}",
        api::status(),
        runtime::status()
    );

    println!(
        "ontolith-server bootstrap metrics: rounds={}, interval_ms={}, ts_ms={}, tx_active={}, tx_begun={}, tx_committed={}, tx_aborted={}, storage_pending={}, storage_ops(triple/quad/delete)={}/{}/{}, storage_write_failures(stage/commit/abort)={}/{}/{}, wal_records={}, points={}, prom_lines={}",
        snapshots.len(),
        sampling_config.interval_ms,
        snapshot.timestamp_ms,
        snapshot.transaction.active,
        snapshot.transaction.begun,
        snapshot.transaction.committed,
        snapshot.transaction.aborted,
        snapshot.storage.pending_transactions,
        snapshot.storage.committed_put_triple_operations,
        snapshot.storage.committed_put_quad_operations,
        snapshot.storage.committed_delete_key_operations,
        snapshot.storage.failed_stage_batches,
        snapshot.storage.failed_commit_transactions,
        snapshot.storage.failed_abort_transactions,
        snapshot.storage.wal_records,
        exported_points.len(),
        prometheus_line_count,
    );
}

fn load_runtime_sampling_config_from_env() -> RuntimeSamplingConfig {
    parse_runtime_sampling_config(
        env::var(METRICS_SAMPLE_ROUNDS_ENV).ok().as_deref(),
        env::var(METRICS_SAMPLE_INTERVAL_MS_ENV).ok().as_deref(),
    )
}

fn parse_runtime_sampling_config(
    rounds_raw: Option<&str>,
    interval_raw: Option<&str>,
) -> RuntimeSamplingConfig {
    let rounds = rounds_raw
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let interval_ms = interval_raw
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    RuntimeSamplingConfig {
        rounds,
        interval_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_runtime_sampling_config;

    #[test]
    fn parse_runtime_sampling_config_uses_defaults_on_missing_values() {
        let config = parse_runtime_sampling_config(None, None);
        assert_eq!(config.rounds, 1);
        assert_eq!(config.interval_ms, 0);
    }

    #[test]
    fn parse_runtime_sampling_config_ignores_invalid_rounds() {
        let config = parse_runtime_sampling_config(Some("0"), Some("15"));
        assert_eq!(config.rounds, 1);
        assert_eq!(config.interval_ms, 15);

        let config = parse_runtime_sampling_config(Some("abc"), Some("3"));
        assert_eq!(config.rounds, 1);
        assert_eq!(config.interval_ms, 3);
    }

    #[test]
    fn parse_runtime_sampling_config_reads_valid_values() {
        let config = parse_runtime_sampling_config(Some("4"), Some("250"));
        assert_eq!(config.rounds, 4);
        assert_eq!(config.interval_ms, 250);
    }
}
