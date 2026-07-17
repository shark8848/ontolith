use ontolith_observability::domain::MetricPoint;
use ontolith_observability::infrastructure::render_prometheus_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiServerConfig {
    pub bind_address: String,
    pub enable_tls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterState {
    pub route_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsEndpoint {
    pub path: &'static str,
}

impl Default for MetricsEndpoint {
    fn default() -> Self {
        Self { path: "/metrics" }
    }
}

pub fn metrics_text(points: &[MetricPoint]) -> String {
    render_prometheus_text(points)
}

pub fn status() -> &'static str {
    "api"
}

#[cfg(test)]
mod tests {
    use super::{metrics_text, MetricsEndpoint};
    use ontolith_observability::domain::{MetricKind, MetricPoint};

    #[test]
    fn metrics_endpoint_defaults_to_prometheus_path() {
        let endpoint = MetricsEndpoint::default();
        assert_eq!(endpoint.path, "/metrics");
    }

    #[test]
    fn metrics_text_renders_prometheus_format() {
        let rendered = metrics_text(&[MetricPoint {
            name: "server.requests.total".to_owned(),
            labels: vec![("route".to_owned(), "/metrics".to_owned())],
            kind: MetricKind::Counter,
            value: 10.0,
            timestamp_ms: 42,
        }]);

        assert!(rendered.contains("# TYPE server_requests_total counter"));
        assert!(rendered.contains("server_requests_total{route=\"/metrics\"} 10 42"));
    }
}
