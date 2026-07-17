#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub worker_threads: usize,
    pub graceful_shutdown_ms: u64,
}

pub fn status() -> &'static str {
    "runtime"
}
