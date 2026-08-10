use crate::{api, app, http, management, runtime};
use ontolith_observability::infrastructure::{
    InMemoryMetricSink, RuntimeSamplingConfig, run_runtime_sampling_loop,
};
use ontolith_security::domain::AuthMode;
use ontolith_storage::infrastructure::InMemoryStorageEngine;
use ontolith_transaction::infrastructure::InMemoryTransactionManager;
use std::env;
#[cfg(feature = "grpc-backend")]
use std::net::SocketAddr;
#[cfg(feature = "grpc-backend")]
use std::sync::Arc;

const METRICS_SAMPLE_ROUNDS_ENV: &str = "ONTOLITH_METRICS_SAMPLE_ROUNDS";
const METRICS_SAMPLE_INTERVAL_MS_ENV: &str = "ONTOLITH_METRICS_SAMPLE_INTERVAL_MS";
#[cfg(feature = "grpc-backend")]
const GRPC_BIND_ENV: &str = "ONTOLITH_GRPC_BIND";
#[cfg(feature = "grpc-backend")]
const DEFAULT_GRPC_BIND: &str = "127.0.0.1:50051";

pub fn run() {
    // ikc-log-center Rust SDK：LOG_CENTER_URL 配置时启用平台日志上报。
    crate::logcenter::init("ontolith-server");

    // Startup diagnostics: one-shot runtime metrics snapshot (legacy bootstrap
    // behavior) so the process logs the same probe as before.
    let tx_manager = InMemoryTransactionManager::new();
    let storage = InMemoryStorageEngine::new();
    let sink = InMemoryMetricSink::new();
    let sampling_config = load_runtime_sampling_config_from_env();
    let snapshots = run_runtime_sampling_loop(&tx_manager, &storage, &sink, sampling_config)
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
    crate::logcenter::emit(
        "INFO",
        "ontolith-server",
        &format!(
            "bootstrap ready: api={}, runtime={}",
            api::status(),
            runtime::status()
        ),
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
    crate::logcenter::emit(
        "INFO",
        "ontolith-server",
        &format!(
            "bootstrap metrics: rounds={}, interval_ms={}, ts_ms={}, tx_active={}, tx_begun={}, tx_committed={}, tx_aborted={}, storage_pending={}, storage_ops(triple/quad/delete)={}/{}/{}, storage_write_failures(stage/commit/abort)={}/{}/{}, wal_records={}, points={}, prom_lines={}",
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
        ),
    );

    // Real gateway (P5-01): shared L5 AppState from the same environment
    // contract as the management server, then HTTP + gRPC access boundaries.
    let app = management::build_gateway_app_state_from_env().unwrap_or_else(|err| {
        eprintln!("ontolith-server startup failed: {err}");
        crate::logcenter::emit(
            "ERROR",
            "ontolith-server",
            &format!("startup failed: {err}"),
        );
        std::process::exit(1);
    });

    #[cfg(feature = "grpc-backend")]
    let grpc_bind = start_grpc(&app);

    #[cfg(not(feature = "grpc-backend"))]
    let grpc_bind = "disabled".to_owned();

    println!(
        "ontolith-server gateway ready: http={}, grpc={}, backend={}, auth_mode={}, tenant_mode={}",
        app.bind_address,
        grpc_bind,
        app.backend.as_str(),
        match app.authenticator.mode {
            AuthMode::Disabled => "disabled",
            AuthMode::Enforced => "enforced",
        },
        app.tenant_mode.as_str(),
    );
    crate::logcenter::emit(
        "INFO",
        "ontolith-server",
        &format!(
            "gateway ready: http={}, grpc={}, backend={}, auth_mode={}, tenant_mode={}",
            app.bind_address,
            grpc_bind,
            app.backend.as_str(),
            match app.authenticator.mode {
                AuthMode::Disabled => "disabled",
                AuthMode::Enforced => "enforced",
            },
            app.tenant_mode.as_str(),
        ),
    );

    let server = http::HttpServer::new(app::shared_handler(app.clone()));
    if let Err(err) = server.serve(&app.bind_address) {
        eprintln!("ontolith-server http listen {}: {err}", app.bind_address);
        crate::logcenter::emit(
            "ERROR",
            "ontolith-server",
            &format!("http listen {}: {err}", app.bind_address),
        );
        std::process::exit(1);
    }
}

#[cfg(feature = "grpc-backend")]
fn start_grpc(app: &Arc<app::AppState>) -> String {
    let raw = env::var(GRPC_BIND_ENV).unwrap_or_else(|_| DEFAULT_GRPC_BIND.to_owned());
    let addr: SocketAddr = match raw.parse() {
        Ok(addr) => addr,
        Err(err) => {
            eprintln!("ontolith-server invalid {GRPC_BIND_ENV} '{raw}': {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = crate::grpc::serve_grpc(app.clone(), addr) {
        eprintln!("ontolith-server gRPC {raw} failed: {err}");
        std::process::exit(1);
    }
    raw
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
