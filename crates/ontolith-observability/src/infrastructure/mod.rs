use crate::application::{
    RuntimeMetricsSnapshot, StorageMetricsReader, TransactionMetricsReader,
    collect_runtime_metrics, runtime_snapshot_to_metric_points,
};
use crate::domain::{MetricKind, MetricPoint, SpanEvent, SpanId, TraceContext, TraceId};
use ontolith_core::error::OntolithError;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

// ---------------------------------------------------------------------------
// Tracing (P5-05): in-memory span store, W3C traceparent context, and a
// thread-local active scope so gateway handlers can record child spans
// without threading context through every handler signature.
// ---------------------------------------------------------------------------

pub trait TraceSink: Send + Sync {
    fn record(&self, span: SpanEvent) -> Result<(), OntolithError>;
}

/// Bounded in-memory span store (newest N spans, oldest evicted).
pub struct InMemoryTraceStore {
    spans: RwLock<Vec<SpanEvent>>,
    cap: usize,
}

impl InMemoryTraceStore {
    pub fn new(cap: usize) -> Self {
        Self {
            spans: RwLock::new(Vec::new()),
            cap: cap.max(1),
        }
    }

    pub fn record(&self, span: SpanEvent) -> Result<(), OntolithError> {
        let mut guard = self
            .spans
            .write()
            .map_err(|_| OntolithError::InvalidState("trace store lock poisoned"))?;
        guard.push(span);
        if guard.len() > self.cap {
            let excess = guard.len() - self.cap;
            guard.drain(..excess);
        }
        Ok(())
    }

    pub fn spans(&self) -> Vec<SpanEvent> {
        self.spans
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.spans.read().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        if let Ok(mut guard) = self.spans.write() {
            guard.clear();
        }
    }
}

impl TraceSink for InMemoryTraceStore {
    fn record(&self, span: SpanEvent) -> Result<(), OntolithError> {
        self.record(span)
    }
}

static TRACE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 64-bit avalanche mix (MurmurHash3 fmix64) for deterministic id generation.
fn mix64(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^= value >> 33;
    value
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 128-bit trace id (32 hex chars, W3C traceparent compatible).
pub fn generate_trace_id() -> TraceId {
    let hi = mix64(now_nanos() ^ TRACE_COUNTER.fetch_add(1, Ordering::Relaxed));
    let lo = mix64(
        now_nanos().rotate_left(32) ^ TRACE_COUNTER.fetch_add(1, Ordering::Relaxed),
    );
    TraceId(format!("{hi:016x}{lo:016x}"))
}

/// 64-bit span id (16 hex chars, W3C traceparent compatible).
pub fn generate_span_id() -> SpanId {
    let value = mix64(now_nanos() ^ TRACE_COUNTER.fetch_add(1, Ordering::Relaxed));
    SpanId(format!("{value:016x}"))
}

/// W3C `traceparent` (version 00, sampled).
pub fn format_traceparent(trace_id: &TraceId, span_id: &SpanId) -> String {
    format!("00-{}-{}-01", trace_id.0, span_id.0)
}

/// Parse a W3C `traceparent` header into a [`TraceContext`]. Returns `None`
/// for malformed headers (wrong version, non-hex, invalid lengths, zero ids).
pub fn parse_traceparent(header: Option<&str>) -> Option<TraceContext> {
    let parts: Vec<&str> = header?.trim().split('-').collect();
    if parts.len() != 4 || parts[0] != "00" {
        return None;
    }
    if parts[1].len() != 32
        || !parts[1].chars().all(|c| c.is_ascii_hexdigit())
        || parts[1].chars().all(|c| c == '0')
    {
        return None;
    }
    if parts[2].len() != 16
        || !parts[2].chars().all(|c| c.is_ascii_hexdigit())
        || parts[2].chars().all(|c| c == '0')
    {
        return None;
    }
    if parts[3].len() != 2 || !parts[3].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(TraceContext {
        trace_id: TraceId(parts[1].to_owned()),
        span_id: SpanId(parts[2].to_owned()),
    })
}

thread_local! {
    static CURRENT_TRACE: RefCell<Option<TraceContext>> = const { RefCell::new(None) };
}

/// RAII guard that installs the active [`TraceContext`] on the current thread
/// and restores the previous context on drop.
pub struct TraceScope {
    previous: Option<TraceContext>,
}

impl TraceScope {
    pub fn enter(context: TraceContext) -> Self {
        let previous = CURRENT_TRACE.with(|cell| cell.borrow_mut().replace(context));
        Self { previous }
    }
}

impl Drop for TraceScope {
    fn drop(&mut self) {
        CURRENT_TRACE.with(|cell| *cell.borrow_mut() = self.previous.clone());
    }
}

pub fn current_trace() -> Option<TraceContext> {
    CURRENT_TRACE.with(|cell| cell.borrow().clone())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render_span_json(span: &SpanEvent) -> String {
    let parent = match &span.parent_span_id {
        Some(id) => json_escape(&id.0),
        None => "null".to_owned(),
    };
    let mut attributes = String::from("{");
    for (idx, (key, value)) in span.attributes.iter().enumerate() {
        if idx > 0 {
            attributes.push(',');
        }
        attributes.push_str(&format!("{}:{}", json_escape(key), json_escape(value)));
    }
    attributes.push('}');
    format!(
        r#"{{"span_id":{},"parent_span_id":{},"name":{},"start_ms":{},"duration_ms":{},"status":{},"attributes":{}}}"#,
        json_escape(&span.span_id.0),
        parent,
        json_escape(&span.name.0),
        span.start_ms,
        span.duration_ms,
        json_escape(span.status.as_str()),
        attributes
    )
}

/// Render spans as `{"traces":[{"trace_id":...,"span_count":N,"spans":[...]}],"total":T}`
/// with newest traces first, each trace's spans ordered by start time.
pub fn render_traces_json(spans: &[SpanEvent], limit: usize) -> String {
    let mut groups: Vec<(TraceId, Vec<&SpanEvent>)> = Vec::new();
    for span in spans {
        match groups.iter_mut().find(|(id, _)| *id == span.trace_id) {
            Some((_, group)) => group.push(span),
            None => groups.push((span.trace_id.clone(), vec![span])),
        }
    }
    groups.sort_by(|a, b| {
        let a_newest = a.1.iter().map(|s| s.start_ms).max().unwrap_or(0);
        let b_newest = b.1.iter().map(|s| s.start_ms).max().unwrap_or(0);
        b_newest.cmp(&a_newest)
    });
    let total = groups.len();

    let mut body = String::from(r#"{"traces":["#);
    for (idx, (trace_id, group)) in groups.into_iter().take(limit).enumerate() {
        if idx > 0 {
            body.push(',');
        }
        body.push_str(&format!(
            r#"{{"trace_id":{},"span_count":{},"spans":["#,
            json_escape(&trace_id.0),
            group.len()
        ));
        let mut sorted: Vec<&&SpanEvent> = group.iter().collect();
        sorted.sort_by_key(|span| span.start_ms);
        for (span_idx, span) in sorted.into_iter().enumerate() {
            if span_idx > 0 {
                body.push(',');
            }
            body.push_str(&render_span_json(span));
        }
        body.push_str("]}");
    }
    body.push_str(&format!(r#"],"total":{total}}}"#));
    body
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
        InMemoryMetricSink, InMemoryTraceStore, MetricSink, RuntimeSamplingConfig, TraceScope,
        collect_and_export_runtime_metrics, current_trace, format_traceparent, generate_span_id,
        generate_trace_id, parse_traceparent, render_prometheus_text, render_traces_json,
        run_runtime_sampling_loop,
    };
    use crate::domain::{MetricKind, MetricPoint, SpanEvent, SpanName, SpanStatus, TraceContext};
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

    #[test]
    fn trace_ids_and_span_ids_are_unique_and_hex() {
        let trace_ids: Vec<_> = (0..8).map(|_| generate_trace_id()).collect();
        let span_ids: Vec<_> = (0..8).map(|_| generate_span_id()).collect();
        let mut uniq_trace: Vec<_> = trace_ids.clone();
        uniq_trace.sort();
        uniq_trace.dedup();
        assert_eq!(uniq_trace.len(), 8, "trace ids must be unique");
        for id in &trace_ids {
            assert_eq!(id.0.len(), 32);
            assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
        }
        let mut uniq_span: Vec<_> = span_ids.clone();
        uniq_span.sort();
        uniq_span.dedup();
        assert_eq!(uniq_span.len(), 8, "span ids must be unique");
        for id in &span_ids {
            assert_eq!(id.0.len(), 16);
            assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn traceparent_roundtrip_and_rejects_malformed() {
        let trace_id = generate_trace_id();
        let span_id = generate_span_id();
        let header = format_traceparent(&trace_id, &span_id);
        let parsed = parse_traceparent(Some(&header)).expect("valid header must parse");
        assert_eq!(parsed.trace_id, trace_id);
        assert_eq!(parsed.span_id, span_id);

        assert!(parse_traceparent(None).is_none());
        assert!(parse_traceparent(Some("01-abcdef-1234-01")).is_none());
        assert!(parse_traceparent(Some("00-xyz-1234567890abcdef-01")).is_none());
        assert!(parse_traceparent(Some("00-00000000000000000000000000000000-1234567890abcdef-01")).is_none());
        assert!(parse_traceparent(Some("00-abcdefabcdefabcdefabcdefabcdefab-0000000000000000-01")).is_none());
        assert!(parse_traceparent(Some("00-abcdefabcdefabcdefabcdefabcdefab-1234567890abcdef-zz")).is_none());
    }

    #[test]
    fn trace_scope_propagates_and_restores() {
        assert!(current_trace().is_none());
        let ctx = TraceContext {
            trace_id: generate_trace_id(),
            span_id: generate_span_id(),
        };
        let scope = TraceScope::enter(ctx.clone());
        assert_eq!(current_trace(), Some(ctx.clone()));

        // Nested scope restores the outer context on drop.
        let inner = TraceContext {
            trace_id: ctx.trace_id.clone(),
            span_id: generate_span_id(),
        };
        let inner_scope = TraceScope::enter(inner.clone());
        assert_eq!(current_trace(), Some(inner));
        drop(inner_scope);
        assert_eq!(current_trace(), Some(ctx.clone()));

        drop(scope);
        assert!(current_trace().is_none());
    }

    #[test]
    fn trace_store_caps_and_evicts_oldest() {
        let store = InMemoryTraceStore::new(3);
        let trace_id = generate_trace_id();
        for idx in 0..5u64 {
            store
                .record(SpanEvent {
                    trace_id: trace_id.clone(),
                    span_id: generate_span_id(),
                    parent_span_id: None,
                    name: SpanName(format!("span.{idx}")),
                    start_ms: idx,
                    duration_ms: 1,
                    status: SpanStatus::Ok,
                    attributes: vec![],
                })
                .expect("record must succeed");
        }
        assert_eq!(store.len(), 3);
        let spans = store.spans();
        assert_eq!(spans[0].name, SpanName("span.2".into()));
        assert_eq!(spans[2].name, SpanName("span.4".into()));
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn render_traces_json_groups_spans_by_trace() {
        let trace_a = generate_trace_id();
        let trace_b = generate_trace_id();
        let root = SpanEvent {
            trace_id: trace_a.clone(),
            span_id: generate_span_id(),
            parent_span_id: None,
            name: SpanName("http.request".into()),
            start_ms: 10,
            duration_ms: 5,
            status: SpanStatus::Ok,
            attributes: vec![("method".into(), "GET".into())],
        };
        let child = SpanEvent {
            trace_id: trace_a.clone(),
            span_id: generate_span_id(),
            parent_span_id: Some(root.span_id.clone()),
            name: SpanName("sparql.execute".into()),
            start_ms: 11,
            duration_ms: 3,
            status: SpanStatus::Error,
            attributes: vec![],
        };
        let other = SpanEvent {
            trace_id: trace_b.clone(),
            span_id: generate_span_id(),
            parent_span_id: None,
            name: SpanName("http.request".into()),
            start_ms: 20,
            duration_ms: 1,
            status: SpanStatus::Ok,
            attributes: vec![],
        };
        let json = render_traces_json(&[root, child, other], 10);
        assert!(json.contains(&format!("\"trace_id\":\"{}\"", trace_b.0)));
        assert!(json.contains(&format!("\"trace_id\":\"{}\"", trace_a.0)));
        assert!(json.contains("\"name\":\"http.request\""));
        assert!(json.contains("\"name\":\"sparql.execute\""));
        assert!(json.contains("\"status\":\"error\""));
        assert!(json.contains("\"method\":\"GET\""));
        assert!(json.contains("\"span_count\":2"));
        assert!(json.contains("\"total\":2"));
        // Newest trace first.
        assert!(
            json.find(&format!("\"trace_id\":\"{}\"", trace_b.0))
                < json.find(&format!("\"trace_id\":\"{}\"", trace_a.0))
        );
    }
}
