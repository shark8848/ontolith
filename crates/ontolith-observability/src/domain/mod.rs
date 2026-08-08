use ontolith_core::domain::TimestampMs;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpanName(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanStatus {
    Ok,
    Error,
}

impl SpanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }
}

/// One recorded span in a distributed trace (P5-05).
#[derive(Debug, Clone, PartialEq)]
pub struct SpanEvent {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    /// Span id of the caller (None for the root span of a trace).
    pub parent_span_id: Option<SpanId>,
    pub name: SpanName,
    pub start_ms: TimestampMs,
    pub duration_ms: u64,
    pub status: SpanStatus,
    pub attributes: Vec<(String, String)>,
}

/// Active trace context propagated across spans on the same execution path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: TraceId,
    /// Span id of the current (or upstream) span.
    pub span_id: SpanId,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub kind: MetricKind,
    pub value: f64,
    pub timestamp_ms: TimestampMs,
}

pub fn status() -> &'static str {
    "domain"
}
