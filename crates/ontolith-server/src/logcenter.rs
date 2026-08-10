//! ikc-log-center 平台日志接入（Rust SDK：`log-center-sdk`）。
//!
//! 进程启动时经 [`init`] 初始化共享客户端，网关 access 日志、启动/错误日志
//! 统一经 [`emit`] / [`emit_access`] 结构化上报到 log-center `/ingest`。
//! 投递为尽力而为（best-effort）：队列满或服务不可达时静默丢弃，不影响业务。

use log_center_sdk::client::LogCenterClient;
use log_center_sdk::config::LogCenterConfig;
use log_center_sdk::entry::LogEntry;
use std::time::Duration;

/// `LOG_CENTER_URL` 未设置或为空时禁用上报（返回 `false`）。
pub fn init(component: &str) -> bool {
    let url = match std::env::var("LOG_CENTER_URL") {
        Ok(url) if !url.trim().is_empty() => url.trim().to_owned(),
        _ => return false,
    };
    let config = config_from_env(component, &url);
    LogCenterClient::initialize(config);
    emit("INFO", component, "log-center sdk enabled");
    true
}

pub fn enabled() -> bool {
    LogCenterClient::shared().is_some()
}

/// 上报一条结构化日志；未初始化时为空操作。
pub fn emit(level: &str, logger: &str, message: &str) {
    let Some(client) = LogCenterClient::shared() else {
        return;
    };
    let _ = client.add(build_entry(level, logger, message, &[]));
}

/// 上报网关 access 日志（结构化字段 method/path/status/latency_ms/bytes）。
pub fn emit_access(method: &str, path: &str, status: u16, latency_ms: u64, bytes: usize) {
    let Some(client) = LogCenterClient::shared() else {
        return;
    };
    let message = format!(
        "access method={method} path={path} status={status} latency_ms={latency_ms} bytes={bytes}"
    );
    let extras = [
        ("method", method),
        ("path", path),
        ("status", &status.to_string()),
        ("latency_ms", &latency_ms.to_string()),
        ("bytes", &bytes.to_string()),
    ];
    let _ = client.add(build_entry(
        "INFO",
        "ontolith-server.access",
        &message,
        &extras,
    ));
}

/// 进程退出前冲刷剩余队列（等待发送线程退出）。
pub fn shutdown() {
    if let Some(client) = LogCenterClient::shared() {
        client.shutdown(Duration::from_secs(2));
    }
}

/// 构建日志条目：附加当前 W3C trace 上下文（trace_id/span_id）。
fn build_entry(level: &str, logger: &str, message: &str, extras: &[(&str, &str)]) -> LogEntry {
    let mut builder = LogEntry::builder()
        .level(level)
        .logger(logger)
        .message(message);
    if let Some(trace) = ontolith_observability::infrastructure::current_trace() {
        builder = builder
            .trace_id(Some(trace.trace_id.0))
            .span_id(Some(trace.span_id.0));
    }
    for (key, value) in extras {
        builder = builder.extra(*key, *value);
    }
    builder.build()
}

/// 与 SDK `LogCenterConfig::from_env` 对齐的环境变量读取，并附加静态字段。
fn config_from_env(component: &str, url: &str) -> LogCenterConfig {
    let mut builder = LogCenterConfig::builder()
        .endpoint(url)
        .token(std::env::var("LOG_CENTER_TOKEN").unwrap_or_default())
        .static_field("app", component)
        .static_field("language", "rust");
    if let Ok(raw) = std::env::var("LOG_CENTER_TIMEOUT")
        && let Ok(secs) = raw.trim().parse::<f64>()
    {
        builder = builder.timeout(Duration::from_millis((secs * 1000.0) as u64));
    }
    if let Ok(raw) = std::env::var("LOG_CENTER_QUEUE")
        && let Ok(n) = raw.trim().parse::<usize>()
    {
        builder = builder.queue_size(n);
    }
    if let Ok(raw) = std::env::var("LOG_CENTER_BATCH")
        && let Ok(n) = raw.trim().parse::<usize>()
    {
        builder = builder.batch_size(n);
    }
    builder
        .build()
        .expect("log-center config build should succeed with endpoint set")
}

#[cfg(test)]
mod tests {
    use super::{build_entry, config_from_env};
    use log_center_sdk::entry::LogEntry;
    use serde_json::Value;

    fn as_map(entry: &LogEntry) -> serde_json::Map<String, Value> {
        entry.as_map().clone()
    }

    #[test]
    fn build_entry_sets_canonical_fields_and_extras() {
        let entry = build_entry(
            "WARNING",
            "ontolith-server.test",
            "boom",
            &[("method", "GET"), ("status", "500")],
        );
        let map = as_map(&entry);
        assert_eq!(map["level"], Value::String("WARNING".into()));
        assert_eq!(map["logger"], Value::String("ontolith-server.test".into()));
        assert_eq!(map["message"], Value::String("boom".into()));
        assert_eq!(map["method"], Value::String("GET".into()));
        assert_eq!(map["status"], Value::String("500".into()));
    }

    #[test]
    fn build_entry_defaults_ts_to_now() {
        let entry = build_entry("INFO", "ontolith-server.test", "hi", &[]);
        let map = as_map(&entry);
        assert!(map["ts"].as_str().is_some_and(|ts| !ts.is_empty()));
    }

    #[test]
    fn config_from_env_adds_static_fields() {
        let config = config_from_env("ontolith-server", "http://127.0.0.1:9315");
        assert_eq!(config.ingest_url(), "http://127.0.0.1:9315/ingest");
        assert_eq!(
            config.static_fields()["app"],
            Value::String("ontolith-server".into())
        );
        assert_eq!(
            config.static_fields()["language"],
            Value::String("rust".into())
        );
    }
}
