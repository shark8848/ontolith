use ontolith_core::domain::TimestampMs;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpanName(pub String);

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
