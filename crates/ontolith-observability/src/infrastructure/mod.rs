use crate::application::{
    RuntimeMetricsSnapshot, StorageMetricsReader, TransactionMetricsReader,
    collect_runtime_metrics, runtime_snapshot_to_metric_points,
};
use crate::domain::{MetricKind, MetricPoint};
use ontolith_core::error::OntolithError;
use std::collections::HashSet;
use std::sync::RwLock;
use std::thread;
use std::time::Duration;

pub trait MetricSink: Send + Sync {
    fn emit(&self, point: MetricPoint) -> Result<(), OntolithError>;

    fn emit_batch(&self, points: &[MetricPoint]) -> Result<(), OntolithError> {
        for point in points {
            self.emit(point.clone())?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryMetricSink {
    points: RwLock<Vec<MetricPoint>>,
}

impl InMemoryMetricSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn points(&self) -> Vec<MetricPoint> {
        self.points
            .read()
            .map(|points| points.clone())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.points.read().map(|points| points.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl MetricSink for InMemoryMetricSink {
    fn emit(&self, point: MetricPoint) -> Result<(), OntolithError> {
        let mut guard = self
            .points
            .write()
            .map_err(|_| OntolithError::InvalidState("metric sink lock poisoned"))?;
        guard.push(point);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSamplingConfig {
    pub rounds: usize,
    pub interval_ms: u64,
}

impl Default for RuntimeSamplingConfig {
    fn default() -> Self {
        Self {
            rounds: 1,
            interval_ms: 0,
        }
    }
}

pub fn export_runtime_snapshot<S>(
    snapshot: &RuntimeMetricsSnapshot,
    sink: &S,
) -> Result<usize, OntolithError>
where
    S: MetricSink,
{
    let points = runtime_snapshot_to_metric_points(snapshot);
    sink.emit_batch(&points)?;
    Ok(points.len())
}

pub fn collect_and_export_runtime_metrics<T, R, S>(
    tx_reader: &T,
    storage_reader: &R,
    sink: &S,
) -> Result<RuntimeMetricsSnapshot, OntolithError>
where
    T: TransactionMetricsReader,
    R: StorageMetricsReader,
    S: MetricSink,
{
    let snapshot = collect_runtime_metrics(tx_reader, storage_reader);
    let _ = export_runtime_snapshot(&snapshot, sink)?;
    Ok(snapshot)
}

pub fn run_runtime_sampling_loop<T, R, S>(
    tx_reader: &T,
    storage_reader: &R,
    sink: &S,
    config: RuntimeSamplingConfig,
) -> Result<Vec<RuntimeMetricsSnapshot>, OntolithError>
where
    T: TransactionMetricsReader,
    R: StorageMetricsReader,
    S: MetricSink,
{
    let rounds = config.rounds.max(1);
    let mut snapshots = Vec::with_capacity(rounds);

    for idx in 0..rounds {
        let snapshot = collect_and_export_runtime_metrics(tx_reader, storage_reader, sink)?;
        snapshots.push(snapshot);

        if idx + 1 < rounds && config.interval_ms > 0 {
            thread::sleep(Duration::from_millis(config.interval_ms));
        }
    }

    Ok(snapshots)
}

pub fn render_prometheus_text(points: &[MetricPoint]) -> String {
    let mut output = String::new();
    let mut typed_metrics = HashSet::new();

    for point in points {
        let metric_name = sanitize_metric_name(&point.name);

        if typed_metrics.insert(metric_name.clone()) {
            output.push_str("# TYPE ");
            output.push_str(&metric_name);
            output.push(' ');
            output.push_str(match point.kind {
                MetricKind::Counter => "counter",
                MetricKind::Gauge => "gauge",
                MetricKind::Histogram => "histogram",
            });
            output.push('\n');
        }

        output.push_str(&metric_name);
        output.push_str(&format_labels(&point.labels));
        output.push(' ');
        output.push_str(&format_float(point.value));
        output.push(' ');
        output.push_str(&point.timestamp_ms.to_string());
        output.push('\n');
    }

    output
}

fn format_labels(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }

    let mut items = labels
        .iter()
        .map(|(key, value)| {
            let sanitized_key = sanitize_metric_name(key);
            let escaped_value = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            format!("{}=\"{}\"", sanitized_key, escaped_value)
        })
        .collect::<Vec<_>>();
    items.sort();
    format!("{{{}}}", items.join(","))
}

fn sanitize_metric_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn format_float(value: f64) -> String {
    let mut text = value.to_string();
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.push('0');
        }
    }
    text
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::{
        InMemoryMetricSink, MetricSink, RuntimeSamplingConfig, collect_and_export_runtime_metrics,
        render_prometheus_text, run_runtime_sampling_loop,
    };
    use crate::domain::{MetricKind, MetricPoint};
    use ontolith_storage::infrastructure::InMemoryStorageEngine;
    use ontolith_transaction::infrastructure::InMemoryTransactionManager;

    #[test]
    fn in_memory_metric_sink_stores_points() {
        let sink = InMemoryMetricSink::new();
        sink.emit(MetricPoint {
            name: "test.counter".to_owned(),
            labels: vec![("component".to_owned(), "test".to_owned())],
            kind: MetricKind::Counter,
            value: 1.0,
            timestamp_ms: 1,
        })
        .expect("emit must succeed");

        assert_eq!(sink.len(), 1);
        assert_eq!(sink.points()[0].name, "test.counter");
    }

    #[test]
    fn collect_and_export_runtime_metrics_publishes_points() {
        let tx_manager = InMemoryTransactionManager::new();
        let storage = InMemoryStorageEngine::new();
        let sink = InMemoryMetricSink::new();

        let snapshot = collect_and_export_runtime_metrics(&tx_manager, &storage, &sink)
            .expect("collection and export must succeed");

        assert!(snapshot.timestamp_ms > 0);
        assert_eq!(sink.len(), 24);
        assert!(
            sink.points()
                .iter()
                .any(|point| point.name == "transaction.active")
        );
    }

    #[test]
    fn runtime_sampling_loop_emits_metrics_for_each_round() {
        let tx_manager = InMemoryTransactionManager::new();
        let storage = InMemoryStorageEngine::new();
        let sink = InMemoryMetricSink::new();

        let snapshots = run_runtime_sampling_loop(
            &tx_manager,
            &storage,
            &sink,
            RuntimeSamplingConfig {
                rounds: 3,
                interval_ms: 0,
            },
        )
        .expect("sampling loop must succeed");

        assert_eq!(snapshots.len(), 3);
        assert_eq!(sink.len(), 72);
        assert!(snapshots[2].timestamp_ms >= snapshots[0].timestamp_ms);
    }

    #[test]
    fn render_prometheus_text_formats_points() {
        let sink = InMemoryMetricSink::new();
        sink.emit(MetricPoint {
            name: "transaction.begun".to_owned(),
            labels: vec![("component".to_owned(), "transaction".to_owned())],
            kind: MetricKind::Counter,
            value: 2.0,
            timestamp_ms: 100,
        })
        .expect("emit first point");
        sink.emit(MetricPoint {
            name: "storage.pending-transactions".to_owned(),
            labels: vec![("component".to_owned(), "storage".to_owned())],
            kind: MetricKind::Gauge,
            value: 1.5,
            timestamp_ms: 101,
        })
        .expect("emit second point");

        let text = render_prometheus_text(&sink.points());
        assert!(text.contains("# TYPE transaction_begun counter"));
        assert!(text.contains("transaction_begun{component=\"transaction\"} 2 100"));
        assert!(text.contains("# TYPE storage_pending_transactions gauge"));
        assert!(text.contains("storage_pending_transactions{component=\"storage\"} 1.5 101"));
    }
}
