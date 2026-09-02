//! Remote HTTP embedding provider (P8-01 RemoteProvider, ADR-0006): external
//! embedding service via HTTP/JSON — OpenAI-compatible `data[].embedding`
//! response shape — with bearer API key, bounded per-request timeout, and a
//! capped FIFO cache keyed by exact input text.
//!
//! The in-tree deterministic [`FeatureHashEmbedding`](super::FeatureHashEmbedding)
//! remains the default provider; this one is opt-in by construction (an
//! endpoint is required). Term projection reuses the shared deterministic
//! `term_text` mapping, so RDF terms stay stable across providers.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;
use serde_json::{Value, json};

use crate::domain::{DEFAULT_EMBEDDING_DIM, Embedding, EmbeddingProvider};
use crate::infrastructure::term_text;

/// Configuration for [`RemoteHttpEmbeddingProvider`].
#[derive(Debug, Clone)]
pub struct RemoteEmbeddingConfig {
    /// Full request URL (e.g. `https://api.openai.com/v1/embeddings`).
    pub endpoint: String,
    /// Optional bearer API key (`Authorization: Bearer <key>`).
    pub api_key: Option<String>,
    /// Model identifier sent in the JSON body.
    pub model: String,
    /// Expected embedding dimension; responses with a different dimension
    /// are rejected so the semantic index layout stays fixed.
    pub dim: usize,
    /// Per-request timeout (connect + read).
    pub timeout: Duration,
    /// Cache capacity in distinct input strings (FIFO eviction).
    pub cache_capacity: usize,
}

impl Default for RemoteEmbeddingConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            api_key: None,
            model: "text-embedding-3-small".to_owned(),
            dim: DEFAULT_EMBEDDING_DIM,
            timeout: Duration::from_secs(30),
            cache_capacity: 4096,
        }
    }
}

/// HTTP/JSON transport boundary for the remote provider (test seam; the
/// production implementation is [`UreqHttpTransport`]).
pub trait RemoteHttpTransport: Send + Sync {
    /// POST `body` as JSON to `url`, returning the parsed JSON response.
    fn post_json(
        &self,
        url: &str,
        api_key: Option<&str>,
        body: Value,
    ) -> Result<Value, OntolithError>;
}

/// Default transport: a `ureq` Agent with the configured timeout.
pub struct UreqHttpTransport {
    agent: ureq::Agent,
}

impl UreqHttpTransport {
    pub fn new(timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(timeout).build(),
        }
    }
}

impl RemoteHttpTransport for UreqHttpTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: Option<&str>,
        body: Value,
    ) -> Result<Value, OntolithError> {
        let mut req = self.agent.post(url);
        if let Some(key) = api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let resp = req
            .send_json(body)
            .map_err(|e| OntolithError::Failed(format!("remote embedding request failed: {e}")))?;
        resp.into_json().map_err(|e| {
            OntolithError::Failed(format!("remote embedding response is not valid JSON: {e}"))
        })
    }
}

/// Bounded FIFO string-keyed embedding cache (drops oldest on overflow).
struct EmbeddingCache {
    entries: HashMap<String, Embedding>,
    order: VecDeque<String>,
    capacity: usize,
}

impl EmbeddingCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn get(&self, key: &str) -> Option<Embedding> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: Embedding) {
        if self.entries.contains_key(&key) {
            return;
        }
        if self.entries.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key.clone(), value);
        self.order.push_back(key);
    }
}

/// External HTTP embedding provider (ADR-0006): API key auth, timeout, cache.
pub struct RemoteHttpEmbeddingProvider {
    config: RemoteEmbeddingConfig,
    transport: Box<dyn RemoteHttpTransport>,
    cache: Mutex<EmbeddingCache>,
}

impl RemoteHttpEmbeddingProvider {
    /// Build with the production `ureq` transport.
    pub fn new(config: RemoteEmbeddingConfig) -> Result<Self, OntolithError> {
        let timeout = config.timeout;
        Self::with_transport(config, Box::new(UreqHttpTransport::new(timeout)))
    }

    /// Build with an injected transport (tests / alternative stacks).
    pub fn with_transport(
        config: RemoteEmbeddingConfig,
        transport: Box<dyn RemoteHttpTransport>,
    ) -> Result<Self, OntolithError> {
        if config.endpoint.is_empty() {
            return Err(OntolithError::InvalidArgument(
                "remote embedding endpoint must not be empty",
            ));
        }
        if config.dim == 0 {
            return Err(OntolithError::InvalidArgument(
                "remote embedding dimension must be non-zero",
            ));
        }
        let cache_capacity = config.cache_capacity;
        Ok(Self {
            config,
            transport,
            cache: Mutex::new(EmbeddingCache::new(cache_capacity)),
        })
    }

    fn fetch(&self, text: &str) -> Result<Embedding, OntolithError> {
        let body = json!({
            "model": self.config.model,
            "input": text,
        });
        let parsed = self.transport.post_json(
            &self.config.endpoint,
            self.config.api_key.as_deref(),
            body,
        )?;
        let values = parsed
            .get("data")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("embedding"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                OntolithError::Failed(
                    "remote embedding response missing data[0].embedding".to_owned(),
                )
            })?;
        let mut dims = Vec::with_capacity(values.len());
        for v in values {
            dims.push(v.as_f64().ok_or_else(|| {
                OntolithError::Failed("remote embedding response has non-numeric values".to_owned())
            })? as f32);
        }
        if dims.len() != self.config.dim {
            return Err(OntolithError::Failed(format!(
                "remote embedding dimension {} != configured {}",
                dims.len(),
                self.config.dim
            )));
        }
        // Normalize like every other provider so cosine reduces to a dot.
        Embedding::new(dims)?.normalized()
    }
}

impl EmbeddingProvider for RemoteHttpEmbeddingProvider {
    fn dim(&self) -> usize {
        self.config.dim
    }

    fn embed_text(&self, text: &str) -> Result<Embedding, OntolithError> {
        if let Some(cached) = self
            .cache
            .lock()
            .expect("embedding cache poisoned")
            .get(text)
        {
            return Ok(cached);
        }
        let embedding = self.fetch(text)?;
        self.cache
            .lock()
            .expect("embedding cache poisoned")
            .insert(text.to_owned(), embedding.clone());
        Ok(embedding)
    }

    fn embed_term(&self, term: &Term) -> Result<Embedding, OntolithError> {
        self.embed_text(&term_text(term))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Deterministic in-memory transport recording every request.
    #[derive(Clone)]
    struct FakeTransport {
        dim: usize,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        error: Option<String>,
    }

    /// Recorded request: (URL, bearer API key, JSON body).
    type RecordedRequest = (String, Option<String>, Value);

    impl FakeTransport {
        fn new(dim: usize) -> Self {
            Self {
                dim,
                requests: Arc::new(Mutex::new(Vec::new())),
                error: None,
            }
        }
    }

    impl RemoteHttpTransport for FakeTransport {
        fn post_json(
            &self,
            url: &str,
            api_key: Option<&str>,
            body: Value,
        ) -> Result<Value, OntolithError> {
            self.requests
                .lock()
                .unwrap()
                .push((url.to_owned(), api_key.map(str::to_owned), body));
            if let Some(e) = &self.error {
                return Err(OntolithError::Failed(e.clone()));
            }
            let values: Vec<f32> = (0..self.dim)
                .map(|i| (i + 1) as f32 / self.dim as f32)
                .collect();
            Ok(json!({ "data": [{ "embedding": values }] }))
        }
    }

    fn provider_with(transport: FakeTransport) -> RemoteHttpEmbeddingProvider {
        RemoteHttpEmbeddingProvider::with_transport(
            RemoteEmbeddingConfig {
                endpoint: "https://embed.example/v1/embeddings".to_owned(),
                api_key: Some("test-key".to_owned()),
                model: "test-model".to_owned(),
                dim: 8,
                ..RemoteEmbeddingConfig::default()
            },
            Box::new(transport),
        )
        .unwrap()
    }

    #[test]
    fn remote_provider_fetches_and_caches() {
        let transport = FakeTransport::new(8);
        let provider = provider_with(transport.clone());
        let a = provider.embed_text("hello world").unwrap();
        let b = provider.embed_text("hello world").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.dim, 8);
        let norm: f32 = a.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "norm={norm}");
        let requests = transport.requests.lock().unwrap();
        // Same input twice -> exactly one HTTP request (cache hit).
        assert_eq!(requests.len(), 1);
        let (url, key, body) = &requests[0];
        assert_eq!(url, "https://embed.example/v1/embeddings");
        assert_eq!(key.as_deref(), Some("test-key"));
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["input"], "hello world");
    }

    #[test]
    fn remote_provider_dimension_mismatch_is_rejected() {
        let transport = FakeTransport::new(16);
        let provider = provider_with(transport);
        let err = provider.embed_text("x").unwrap_err();
        assert!(err.message().contains("dimension"), "err={err}");
    }

    #[test]
    fn remote_provider_propagates_transport_error() {
        let mut transport = FakeTransport::new(8);
        transport.error = Some("boom".to_owned());
        let provider = provider_with(transport);
        let err = provider.embed_text("x").unwrap_err();
        assert!(err.message().contains("boom"), "err={err}");
    }

    #[test]
    fn remote_provider_rejects_empty_endpoint() {
        let err = RemoteHttpEmbeddingProvider::new(RemoteEmbeddingConfig::default())
            .err()
            .expect("empty endpoint must be rejected");
        assert!(err.message().contains("endpoint"), "err={err}");
    }
}
