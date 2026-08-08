//! Application state and route handlers for L5 HTTP gateway.

use crate::http::{HttpRequest, HttpResponse, now_ms};
use crate::reasoning::{
    InferenceConfig, ReasoningReadService, base_read_service, reasoning_input,
};
use ontolith_cluster::application::ClusterRuntime;
use ontolith_cluster::domain::{ClusterNodeId, LogPayload, SessionId};
use ontolith_cluster::infrastructure::{ClusterConfig, InMemoryClusterRuntime};
#[cfg(feature = "raft-backend")]
use ontolith_cluster::infrastructure::raft::{RaftClusterConfig, RaftClusterRuntime};
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;
use ontolith_observability::domain::{
    MetricKind, MetricPoint, SpanEvent, SpanName, SpanStatus, TraceContext,
};
use ontolith_observability::infrastructure::{
    InMemoryMetricSink, InMemoryTraceStore, TraceScope, current_trace, format_traceparent,
    generate_span_id, generate_trace_id, parse_traceparent, render_prometheus_text,
};
use ontolith_parser::domain::ParseFormat;
use ontolith_parser::infrastructure::{
    parse_nquads, parse_ntriples, parse_trig_doc, parse_turtle_doc,
};
use ontolith_query::domain::{
    BoundValue, PatternCost, QueryExplain, QueryKind, QueryRequest, QueryResult,
};
use ontolith_query::infrastructure::{update_pipeline, update_pipeline_with_read};
use ontolith_reasoner::application::Reasoner;
use ontolith_reasoner::domain::{InferenceMode, ReasoningReport};
use ontolith_reasoner::infrastructure::ForwardChainReasoner;
use ontolith_rdf::domain::Triple;
use ontolith_security::application::{
    Authenticator, HeaderAuthenticator, InMemoryAuditLog, authorize,
};
use ontolith_security::domain::{
    AuditOutcome, AuthContext, AuthMode, TenantMode, TenantNamespace,
};
use ontolith_storage::application::{DictionaryCodec, StorageEngine, TripleRepository};
use ontolith_storage::domain::{StorageStats, WriteBatch, WriteOperation};
use ontolith_storage::infrastructure::{
    EngineTripleRepository, InMemoryDictionary, InMemoryStorageEngine,
};
use ontolith_transaction::application::TransactionManager;
use ontolith_transaction::domain::TxnMode;
use ontolith_transaction::infrastructure::InMemoryTransactionManager;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Storage backend kind selected at bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBackendKind {
    Memory,
    RocksDb,
}

impl StorageBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::RocksDb => "rocksdb",
        }
    }
}

pub struct AppState {
    pub storage: Arc<dyn StorageEngine>,
    pub dictionary: Arc<dyn DictionaryCodec>,
    pub triples: Arc<dyn TripleRepository>,
    pub txns: Arc<InMemoryTransactionManager>,
    pub authenticator: HeaderAuthenticator,
    pub audit: InMemoryAuditLog,
    pub metrics: InMemoryMetricSink,
    pub traces: InMemoryTraceStore,
    pub requests_total: AtomicU64,
    pub sparql_total: AtomicU64,
    pub sparql_errors: AtomicU64,
    pub ingest_total: AtomicU64,
    pub latency_sum_ms: AtomicU64,
    pub latency_count: AtomicU64,
    pub status_counts: std::sync::Mutex<HashMap<u16, u64>>,
    pub bind_address: String,
    pub backend: StorageBackendKind,
    pub data_dir: Option<PathBuf>,
    /// Tenant isolation posture (P5-03): `Enforced` scopes every read/write
    /// to the caller's tenant namespace.
    pub tenant_mode: TenantMode,
    /// L6 reasoning posture (P6-03): inference mode + materialization guards
    /// applied to the shared SPARQL execution path.
    pub inference: InferenceConfig,
    pub cluster: Arc<dyn ClusterRuntime>,
    pub cluster_tick: AtomicU64,
}

impl AppState {
    pub fn new_memory(bind_address: String, auth: HeaderAuthenticator) -> Arc<Self> {
        Self::new_memory_with_audit(
            bind_address,
            auth,
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
        )
    }

    pub fn new_memory_with_audit(
        bind_address: String,
        auth: HeaderAuthenticator,
        audit: InMemoryAuditLog,
        tenant_mode: TenantMode,
    ) -> Arc<Self> {
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let dictionary: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        Self::from_parts(
            storage,
            dictionary,
            triples,
            bind_address,
            auth,
            StorageBackendKind::Memory,
            None,
            default_cluster(),
            audit,
            tenant_mode,
            InferenceConfig::default(),
        )
    }

    /// Build a memory-storage `AppState` with an explicit cluster runtime
    /// (used by the management binary when `ONTOLITH_CLUSTER_MODE` selects
    /// the raft-backed runtime).
    pub fn new_memory_with_cluster(
        bind_address: String,
        auth: HeaderAuthenticator,
        audit: InMemoryAuditLog,
        tenant_mode: TenantMode,
        cluster: Arc<dyn ClusterRuntime>,
        inference: InferenceConfig,
    ) -> Arc<Self> {
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let dictionary: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        Self::from_parts(
            storage,
            dictionary,
            triples,
            bind_address,
            auth,
            StorageBackendKind::Memory,
            None,
            cluster,
            audit,
            tenant_mode,
            inference,
        )
    }

    #[cfg(feature = "rocksdb-backend")]
    pub fn new_rocksdb(
        bind_address: String,
        auth: HeaderAuthenticator,
        path: PathBuf,
    ) -> Result<Arc<Self>, OntolithError> {
        Self::new_rocksdb_with_audit(
            bind_address,
            auth,
            path,
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
        )
    }

    #[cfg(feature = "rocksdb-backend")]
    pub fn new_rocksdb_with_audit(
        bind_address: String,
        auth: HeaderAuthenticator,
        path: PathBuf,
        audit: InMemoryAuditLog,
        tenant_mode: TenantMode,
    ) -> Result<Arc<Self>, OntolithError> {
        let engine = Arc::new(ontolith_storage::open_durable_engine(&path)?);
        let dictionary: Arc<dyn DictionaryCodec> = Arc::clone(&engine) as Arc<dyn DictionaryCodec>;
        let storage: Arc<dyn StorageEngine> = Arc::clone(&engine) as Arc<dyn StorageEngine>;
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        Ok(Self::from_parts(
            storage,
            dictionary,
            triples,
            bind_address,
            auth,
            StorageBackendKind::RocksDb,
            Some(path),
            default_cluster(),
            audit,
            tenant_mode,
            InferenceConfig::default(),
        ))
    }

    /// RocksDB-storage variant with an explicit cluster runtime (raft mode).
    #[cfg(feature = "rocksdb-backend")]
    pub fn new_rocksdb_with_cluster(
        bind_address: String,
        auth: HeaderAuthenticator,
        path: PathBuf,
        audit: InMemoryAuditLog,
        tenant_mode: TenantMode,
        cluster: Arc<dyn ClusterRuntime>,
        inference: InferenceConfig,
    ) -> Result<Arc<Self>, OntolithError> {
        let engine = Arc::new(ontolith_storage::open_durable_engine(&path)?);
        let dictionary: Arc<dyn DictionaryCodec> = Arc::clone(&engine) as Arc<dyn DictionaryCodec>;
        let storage: Arc<dyn StorageEngine> = Arc::clone(&engine) as Arc<dyn StorageEngine>;
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        Ok(Self::from_parts(
            storage,
            dictionary,
            triples,
            bind_address,
            auth,
            StorageBackendKind::RocksDb,
            Some(path),
            cluster,
            audit,
            tenant_mode,
            inference,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        storage: Arc<dyn StorageEngine>,
        dictionary: Arc<dyn DictionaryCodec>,
        triples: Arc<dyn TripleRepository>,
        bind_address: String,
        auth: HeaderAuthenticator,
        backend: StorageBackendKind,
        data_dir: Option<PathBuf>,
        cluster: Arc<dyn ClusterRuntime>,
        audit: InMemoryAuditLog,
        tenant_mode: TenantMode,
        inference: InferenceConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            storage,
            dictionary,
            triples,
            txns: Arc::new(InMemoryTransactionManager::new()),
            authenticator: auth,
            audit,
            metrics: InMemoryMetricSink::new(),
            traces: InMemoryTraceStore::new(1024),
            requests_total: AtomicU64::new(0),
            sparql_total: AtomicU64::new(0),
            sparql_errors: AtomicU64::new(0),
            ingest_total: AtomicU64::new(0),
            latency_sum_ms: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            status_counts: std::sync::Mutex::new(HashMap::new()),
            bind_address,
            backend,
            data_dir,
            tenant_mode,
            inference,
            cluster,
            cluster_tick: AtomicU64::new(0),
        })
    }

    pub fn handle(self: &Arc<Self>, req: HttpRequest) -> HttpResponse {
        let started = Instant::now();
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        let method = req.method.to_ascii_uppercase();
        let path = req.path.as_str();

        if method == "OPTIONS" {
            return cors(HttpResponse::text(204, "No Content", ""));
        }

        // Tracing (P5-05): continue an upstream W3C `traceparent` context or
        // start a new trace; the root span is recorded after dispatch and the
        // context is echoed back so downstream hops continue the same trace.
        let upstream = parse_traceparent(req.header("traceparent"));
        let trace_id = upstream
            .as_ref()
            .map(|ctx| ctx.trace_id.clone())
            .unwrap_or_else(generate_trace_id);
        let root_span_id = generate_span_id();
        let parent_span_id = upstream.as_ref().map(|ctx| ctx.span_id.clone());
        let _scope = TraceScope::enter(TraceContext {
            trace_id: trace_id.clone(),
            span_id: root_span_id.clone(),
        });
        let started_at_ms = now_ms();

        let result = match (method.as_str(), path) {
            ("GET", "/health") | ("GET", "/healthz") => self.health(&req),
            ("GET", "/ready") | ("GET", "/readyz") => self.ready(&req),
            ("GET", "/metrics") => self.metrics_route(&req),
            ("GET", "/audit") => self.audit_route(&req),
            ("GET", "/sparql") | ("POST", "/sparql") => self.sparql(&req, false),
            ("GET", "/explain") | ("POST", "/explain") => self.sparql(&req, true),
            ("POST", "/data")
            | ("POST", "/data/nt")
            | ("POST", "/data/turtle")
            | ("POST", "/data/trig")
            | ("POST", "/data/nq") => self.ingest(&req, path),
            ("GET", "/cluster") | ("GET", "/cluster/status") => self.cluster_status(&req),
            ("GET", "/cluster/membership") => self.cluster_membership(&req),
            ("GET", "/cluster/shards") => self.cluster_shards(&req),
            ("GET", "/cluster/route") => self.cluster_route(&req),
            ("POST", "/cluster/heartbeat") => self.cluster_heartbeat(&req),
            ("POST", "/cluster/tick") => self.cluster_tick(&req),
            ("POST", "/cluster/replicate") => self.cluster_replicate(&req),
            ("POST", "/cluster/rebalance") => self.cluster_rebalance(&req),
            ("POST", "/cluster/partition") => self.cluster_partition(&req),
            ("POST", "/cluster/heal") => self.cluster_heal(&req),
            ("GET", "/cluster/failover") => self.cluster_failover_history(&req),
            _ => Ok(HttpResponse::json(
                404,
                "Not Found",
                r#"{"error":"not_found"}"#,
            )),
        };

        let resp = match result {
            Ok(resp) => cors(resp),
            Err(err) => {
                if path.contains("sparql") || path.contains("explain") {
                    self.sparql_errors.fetch_add(1, Ordering::Relaxed);
                }
                cors(error_response(err))
            }
        };

        let elapsed = started.elapsed().as_millis() as u64;
        self.latency_sum_ms.fetch_add(elapsed, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut map) = self.status_counts.lock() {
            *map.entry(resp.status).or_insert(0) += 1;
        }
        // Record the request root span and propagate the trace context.
        let _ = self.traces.record(SpanEvent {
            trace_id: trace_id.clone(),
            span_id: root_span_id.clone(),
            parent_span_id,
            name: SpanName("http.request".into()),
            start_ms: started_at_ms,
            duration_ms: elapsed,
            status: if resp.status >= 400 {
                SpanStatus::Error
            } else {
                SpanStatus::Ok
            },
            attributes: vec![
                ("method".into(), method.clone()),
                ("path".into(), path.to_owned()),
                ("status".into(), resp.status.to_string()),
                ("latency_ms".into(), elapsed.to_string()),
            ],
        });
        let mut resp = resp;
        resp.headers.push((
            "Traceparent".into(),
            format_traceparent(&trace_id, &root_span_id),
        ));
        // Request access log line (structured-ish plain text).
        eprintln!(
            "access method={} path={} status={} latency_ms={} bytes={}",
            method,
            path,
            resp.status,
            elapsed,
            resp.body.len()
        );
        resp
    }

    fn auth(&self, req: &HttpRequest) -> Result<AuthContext, OntolithError> {
        let start_ms = now_ms();
        let result = self.authenticator.authenticate_with_bearer(
            req.header("x-ontolith-tenant"),
            req.header("x-ontolith-user"),
            req.header("x-api-key"),
            req.header("authorization"),
        );
        if let Some(trace) = current_trace() {
            let _ = self.traces.record(SpanEvent {
                trace_id: trace.trace_id,
                span_id: generate_span_id(),
                parent_span_id: Some(trace.span_id),
                name: SpanName("http.auth".into()),
                start_ms,
                duration_ms: now_ms() - start_ms,
                status: if result.is_ok() {
                    SpanStatus::Ok
                } else {
                    SpanStatus::Error
                },
                attributes: vec![],
            });
        }
        result
    }

    fn health(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let stats = self.storage.stats();
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"status":"ok","layer":"L5","bind":{},"backend":{},"triples":{},"quads":{},"pending_txns":{},"auth_mode":{},"tenant_mode":{},"jwt":{},"tracing":"on","data_dir":{}}}"#,
                json_string(&self.bind_address),
                json_string(self.backend.as_str()),
                stats.triple_count,
                stats.quad_count,
                stats.pending_transactions,
                json_string(match self.authenticator.mode {
                    AuthMode::Disabled => "disabled",
                    AuthMode::Enforced => "enforced",
                }),
                json_string(self.tenant_mode.as_str()),
                json_string(match &self.authenticator.jwt_secret {
                    Some(_) => "on",
                    None => "off",
                }),
                match &self.data_dir {
                    Some(p) => json_string(&p.display().to_string()),
                    None => "null".into(),
                }
            ),
        ))
    }

    fn ready(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        // Readiness: storage stats callable.
        let _ = self.storage.stats();
        Ok(HttpResponse::json(200, "OK", r#"{"status":"ready"}"#))
    }

    fn metrics_route(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "metrics", "read", now_ms())?;
        let ts = now_ms();
        let mut points = self.metrics.points();
        let push = |points: &mut Vec<MetricPoint>, name: &str, kind: MetricKind, value: f64| {
            points.push(MetricPoint {
                name: name.into(),
                labels: vec![],
                kind,
                value,
                timestamp_ms: ts,
            });
        };
        push(
            &mut points,
            "ontolith_http_requests_total",
            MetricKind::Counter,
            self.requests_total.load(Ordering::Relaxed) as f64,
        );
        push(
            &mut points,
            "ontolith_sparql_requests_total",
            MetricKind::Counter,
            self.sparql_total.load(Ordering::Relaxed) as f64,
        );
        push(
            &mut points,
            "ontolith_sparql_errors_total",
            MetricKind::Counter,
            self.sparql_errors.load(Ordering::Relaxed) as f64,
        );
        push(
            &mut points,
            "ontolith_ingest_requests_total",
            MetricKind::Counter,
            self.ingest_total.load(Ordering::Relaxed) as f64,
        );
        let lat_count = self.latency_count.load(Ordering::Relaxed);
        let lat_sum = self.latency_sum_ms.load(Ordering::Relaxed);
        push(
            &mut points,
            "ontolith_http_request_latency_ms_sum",
            MetricKind::Counter,
            lat_sum as f64,
        );
        push(
            &mut points,
            "ontolith_http_request_latency_ms_count",
            MetricKind::Counter,
            lat_count as f64,
        );
        if lat_count > 0 {
            push(
                &mut points,
                "ontolith_http_request_latency_ms_avg",
                MetricKind::Gauge,
                lat_sum as f64 / lat_count as f64,
            );
        }
        let stats: StorageStats = self.storage.stats();
        push(
            &mut points,
            "ontolith_storage_triples",
            MetricKind::Gauge,
            stats.triple_count as f64,
        );
        push(
            &mut points,
            "ontolith_storage_quads",
            MetricKind::Gauge,
            stats.quad_count as f64,
        );
        push(
            &mut points,
            "ontolith_storage_pending_txns",
            MetricKind::Gauge,
            stats.pending_transactions as f64,
        );
        push(
            &mut points,
            "ontolith_audit_events",
            MetricKind::Gauge,
            self.audit.len() as f64,
        );
        if let Ok(map) = self.status_counts.lock() {
            for (status, count) in map.iter() {
                points.push(MetricPoint {
                    name: "ontolith_http_responses_total".into(),
                    labels: vec![("status".into(), status.to_string())],
                    kind: MetricKind::Counter,
                    value: *count as f64,
                    timestamp_ms: ts,
                });
            }
        }
        Ok(HttpResponse::html_like_prometheus(render_prometheus_text(
            &points,
        )))
    }

    fn audit_route(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "metrics", "read", now_ms())?;
        let limit = req
            .query
            .get("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100)
            .min(1000);
        let mut events = if ctx.tenant.as_str() == "system" {
            self.audit.events()
        } else {
            self.audit.by_tenant(&ctx.tenant)
        };
        if events.len() > limit {
            events = events.split_off(events.len() - limit);
        }
        let mut body = String::from("[");
        for (i, e) in events.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#"{{"ts":{},"tenant":{},"user":{},"action":{},"resource":{},"outcome":{},"detail":{}}}"#,
                e.timestamp_ms,
                json_string(&e.tenant),
                json_string(&e.user),
                json_string(&e.action),
                json_string(&e.resource),
                json_string(e.outcome.as_str()),
                json_string(&e.detail),
            ));
        }
        body.push(']');
        Ok(HttpResponse::json(200, "OK", body))
    }

    fn sparql(
        &self,
        req: &HttpRequest,
        force_explain: bool,
    ) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        let action = if force_explain { "explain" } else { "query" };
        authorize(&self.audit, &ctx, "sparql", action, now_ms())?;
        self.sparql_total.fetch_add(1, Ordering::Relaxed);

        let query_text = extract_sparql_query(req)?;
        if query_text.trim().is_empty() {
            return Err(OntolithError::InvalidArgument("missing SPARQL query"));
        }

        let explain = force_explain
            || req
                .query
                .get("explain")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false)
            || req
                .header("x-ontolith-explain")
                .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

        let timeout_ms = req
            .query
            .get("timeout_ms")
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                req.header("x-ontolith-timeout-ms")
                    .and_then(|v| v.parse().ok())
            });

        let consistency = req
            .header("x-ontolith-consistency")
            .map(parse_consistency)
            .unwrap_or(ConsistencyLevel::Strong);

        let format = req
            .query
            .get("format")
            .map(|s| s.as_str())
            .or_else(|| req.header("accept"))
            .unwrap_or("json");

        // Per-request inference mode override (P6-03): `?inference=off|forward|hybrid`.
        let effective_inference = match req.query.get("inference").map(|s| s.as_str()) {
            Some(raw) => self.inference.with_override(raw)?,
            None => self.inference,
        };

        let start_ms = now_ms();
        let executed = (|| -> Result<HttpResponse, OntolithError> {
            if explain {
                let plan = self.explain_sparql(&ctx, &query_text, timeout_ms, consistency)?;
                let body = explain_json(&plan, ctx.tenant.as_str(), consistency);
                return Ok(HttpResponse::json(200, "OK", body));
            }

            let outcome = self.execute_sparql_with_inference(
                &ctx,
                &query_text,
                timeout_ms,
                consistency,
                &effective_inference,
            )?;

            // SPARQL Query Results JSON Format (W3C-inspired) when accept/format asks for it.
            if format.contains("sparql-results") || format == "srj" || format == "json" {
                return Ok(HttpResponse::json(
                    200,
                    "OK",
                    sparql_results_json(
                        &outcome.result,
                        &ctx,
                        consistency,
                        outcome.reasoning.as_ref(),
                    ),
                ));
            }
            Ok(HttpResponse::json(
                200,
                "OK",
                sparql_results_json(
                    &outcome.result,
                    &ctx,
                    consistency,
                    outcome.reasoning.as_ref(),
                ),
            ))
        })();
        if let Some(trace) = current_trace() {
            let _ = self.traces.record(SpanEvent {
                trace_id: trace.trace_id,
                span_id: generate_span_id(),
                parent_span_id: Some(trace.span_id),
                name: SpanName("sparql.execute".into()),
                start_ms,
                duration_ms: now_ms() - start_ms,
                status: if executed.is_ok() {
                    SpanStatus::Ok
                } else {
                    SpanStatus::Error
                },
                attributes: vec![
                    (
                        "kind".into(),
                        (if explain { "explain" } else { "query" }).to_owned(),
                    ),
                    ("tenant".into(), ctx.tenant.as_str().to_owned()),
                ],
            });
        }
        executed
    }

    /// Build a tenant-scoped [`QueryRequest`] from transport-level inputs.
    fn build_query_request(
        &self,
        ctx: &AuthContext,
        query_text: &str,
        timeout_ms: Option<u64>,
        consistency: ConsistencyLevel,
    ) -> QueryRequest {
        let mut qreq = QueryRequest::new(query_text.to_owned()).with_consistency(consistency);
        qreq.tenant = Some(ctx.tenant.as_str().to_owned());
        if self.tenant_mode == TenantMode::Enforced {
            qreq = qreq.with_tenant_scope(ontolith_query::domain::TenantScope::new(
                ctx.tenant.as_str().to_owned(),
            ));
        }
        if let Some(t) = timeout_ms {
            qreq = qreq.with_timeout(t);
        }
        qreq
    }

    /// Execute a SPARQL query/update with an explicit inference posture
    /// (P6-03). Reads materialize the OWL 2 RL closure over the tenant's
    /// triples and run against an overlay read service; updates and explains
    /// skip materialization. The reasoning report is surfaced in the response.
    pub(crate) fn execute_sparql_with_inference(
        &self,
        ctx: &AuthContext,
        query_text: &str,
        timeout_ms: Option<u64>,
        consistency: ConsistencyLevel,
        inference: &InferenceConfig,
    ) -> Result<SparqlOutcome, OntolithError> {
        let qreq = self.build_query_request(ctx, query_text, timeout_ms, consistency);
        let pipeline = update_pipeline(
            Arc::clone(&self.triples),
            Arc::clone(&self.storage),
            Some(Arc::clone(&self.dictionary)),
        );
        let plan = pipeline.plan(&qreq)?;
        let (reasoning, result) = if inference.is_enabled() && plan.kind != QueryKind::Update {
            let base = base_read_service(
                Arc::clone(&self.triples),
                Arc::clone(&self.dictionary),
                Arc::clone(&self.storage),
            );
            let input = reasoning_input(base.as_ref(), qreq.tenant_scope.as_ref())?;
            let task = inference.reasoning_task(Some(plan.id));
            let outcome = ForwardChainReasoner::new()
                .materialize(self.dictionary.as_ref(), &task, &input)?;
            // Overlay only the increment so base triples are not duplicated.
            let inferred_only: Vec<Triple> = outcome
                .triples
                .iter()
                .filter(|t| !input.contains(t))
                .cloned()
                .collect();
            let overlay = Arc::new(ReasoningReadService::new(base, inferred_only));
            let reasoning_pipeline = update_pipeline_with_read(overlay, Arc::clone(&self.storage));
            (
                Some(ReasoningExecution {
                    mode: inference.mode,
                    report: outcome.report,
                }),
                reasoning_pipeline.execute_planned(&plan, &qreq),
            )
        } else {
            (None, pipeline.execute_planned(&plan, &qreq))
        };
        let result = result?;
        self.audit.record(
            now_ms(),
            ctx,
            "query",
            "sparql",
            AuditOutcome::Allow,
            if result.kind == QueryKind::Update {
                format!("affected={}", result.affected)
            } else {
                format!("rows={}", result.row_count())
            },
        );
        Ok(SparqlOutcome { result, reasoning })
    }

    /// Explain a SPARQL query through the shared pipeline (P5-01).
    pub(crate) fn explain_sparql(
        &self,
        ctx: &AuthContext,
        query_text: &str,
        timeout_ms: Option<u64>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryExplain, OntolithError> {
        let qreq = self.build_query_request(ctx, query_text, timeout_ms, consistency);
        let pipeline = update_pipeline(
            Arc::clone(&self.triples),
            Arc::clone(&self.storage),
            Some(Arc::clone(&self.dictionary)),
        );
        let plan = pipeline.explain(&qreq)?;
        self.audit.record(
            now_ms(),
            ctx,
            "explain",
            "sparql",
            AuditOutcome::Allow,
            format!("plan={}", plan.plan_id.0),
        );
        Ok(plan)
    }

    fn ingest(&self, req: &HttpRequest, path: &str) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "data", "write", now_ms())?;
        self.ingest_total.fetch_add(1, Ordering::Relaxed);

        let start_ms = now_ms();
        let ingested = (|| -> Result<HttpResponse, OntolithError> {
            let format = detect_ingest_format(req, path)?;
            let text = req.body_str();
            if text.trim().is_empty() {
                return Err(OntolithError::InvalidArgument("empty ingest body"));
            }

            let dict = self.dictionary.as_ref();
            let parsed = match format {
                ParseFormat::NTriples => parse_ntriples(text, dict)?,
                ParseFormat::NQuads => parse_nquads(text, dict)?,
                ParseFormat::Turtle => parse_turtle_doc(text, dict)?,
                ParseFormat::TriG => parse_trig_doc(text, dict)?,
                ParseFormat::JsonLd => {
                    return Err(OntolithError::Unsupported("json-ld"));
                }
            };

            // Tenant isolation at write path (P5-03): enforced mode ALWAYS
            // stamps default-graph statements into the tenant's graph and
            // rejects any graph reference outside the tenant namespace.
            let namespace = TenantNamespace::new(ctx.tenant.as_str());
            let tenant_graph = if self.tenant_mode == TenantMode::Enforced {
                match req.query.get("graph") {
                    Some(g) => {
                        namespace.require_owned(g)?;
                        Some(g.clone())
                    }
                    None => Some(namespace.tenant_graph()),
                }
            } else {
                req.query.get("graph").cloned().or_else(|| {
                    if req
                        .query
                        .get("tenant_graph")
                        .map(|v| v == "1" || v == "true")
                        .unwrap_or(false)
                    {
                        Some(namespace.tenant_graph())
                    } else {
                        None
                    }
                })
            };

        let mut ops = Vec::new();
        for t in parsed.dataset.default_graph {
            if let Some(g) = &tenant_graph {
                ops.push(WriteOperation::PutQuad(
                    ontolith_rdf::domain::Quad::in_named_graph(
                        t,
                        ontolith_core::domain::Iri::new(g.clone()),
                    ),
                ));
            } else {
                ops.push(WriteOperation::PutTriple(t));
            }
        }
        for ng in parsed.dataset.named_graphs {
            if self.tenant_mode == TenantMode::Enforced {
                namespace.require_owned(ng.name.as_str())?;
            }
            for t in ng.triples {
                ops.push(WriteOperation::PutQuad(
                    ontolith_rdf::domain::Quad::in_named_graph(t, ng.name.clone()),
                ));
            }
        }
        if ops.is_empty() {
            return Err(OntolithError::InvalidArgument("no statements parsed"));
        }

        let txn = self.txns.begin(TxnMode::ReadWrite)?;
        self.storage.apply_write_batch(&WriteBatch {
            txn_id: txn.id,
            operations: ops.clone(),
        })?;
        self.storage.commit_transaction(txn.id)?;
        let _ = self.txns.commit(txn.id);

        let triple_n = ops
            .iter()
            .filter(|o| matches!(o, WriteOperation::PutTriple(_)))
            .count();
        let quad_n = ops
            .iter()
            .filter(|o| matches!(o, WriteOperation::PutQuad(_)))
            .count();

        self.audit.record(
            now_ms(),
            &ctx,
            "write",
            "data",
            AuditOutcome::Allow,
            format!(
                "format={} triples={} quads={}",
                format.as_str(),
                triple_n,
                quad_n
            ),
        );

            Ok(HttpResponse::json(
                200,
                "OK",
                format!(
                    r#"{{"format":{},"triples":{},"quads":{},"tenant":{},"graph":{}}}"#,
                    json_string(format.as_str()),
                    triple_n,
                    quad_n,
                    json_string(ctx.tenant.as_str()),
                    match tenant_graph {
                        Some(g) => json_string(&g),
                        None => "null".into(),
                    }
                ),
            ))
        })();
        if let Some(trace) = current_trace() {
            let _ = self.traces.record(SpanEvent {
                trace_id: trace.trace_id,
                span_id: generate_span_id(),
                parent_span_id: Some(trace.span_id),
                name: SpanName("data.ingest".into()),
                start_ms,
                duration_ms: now_ms() - start_ms,
                status: if ingested.is_ok() {
                    SpanStatus::Ok
                } else {
                    SpanStatus::Error
                },
                attributes: vec![
                    ("path".into(), path.to_owned()),
                    ("tenant".into(), ctx.tenant.as_str().to_owned()),
                ],
            });
        }
        ingested
    }

    // ---- L4 cluster control plane HTTP ----

    fn cluster_status(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let st = self.cluster.status();
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"epoch":{},"leader":{},"nodes":{},"healthy":{},"shards":{},"log_index":{},"commit_index":{},"failovers":{},"partition":{}}}"#,
                st.epoch.get(),
                st.leader_id
                    .as_ref()
                    .map(|l| json_string(l.as_str()))
                    .unwrap_or_else(|| "null".into()),
                st.node_count,
                st.healthy_count,
                st.shard_count,
                st.leader_log_index,
                st.commit_index,
                st.failover_count,
                st.partition_active,
            ),
        ))
    }

    fn cluster_membership(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let m = self.cluster.membership();
        let mut nodes = String::from("[");
        for (i, n) in m.nodes.iter().enumerate() {
            if i > 0 {
                nodes.push(',');
            }
            nodes.push_str(&format!(
                r#"{{"id":{},"address":{},"role":{},"status":{},"heartbeat":{}}}"#,
                json_string(n.node_id.as_str()),
                json_string(&n.address),
                json_string(n.role.as_str()),
                json_string(n.status.as_str()),
                n.last_heartbeat,
            ));
        }
        nodes.push(']');
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"epoch":{},"leader":{},"nodes":{nodes}}}"#,
                m.epoch.get(),
                m.leader_id
                    .as_ref()
                    .map(|l| json_string(l.as_str()))
                    .unwrap_or_else(|| "null".into()),
            ),
        ))
    }

    fn cluster_shards(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let map = self.cluster.shard_map();
        let mut assignments = String::from("[");
        for (i, a) in map.assignments.iter().enumerate() {
            if i > 0 {
                assignments.push(',');
            }
            let leader = a
                .replica_set
                .leader_id
                .as_ref()
                .map(|l| json_string(l.as_str()))
                .unwrap_or_else(|| "null".into());
            let mut followers = String::from("[");
            for (j, f) in a.replica_set.follower_ids.iter().enumerate() {
                if j > 0 {
                    followers.push(',');
                }
                followers.push_str(&json_string(f.as_str()));
            }
            followers.push(']');
            assignments.push_str(&format!(
                r#"{{"shard":{},"slots":[{},{}],"leader":{},"followers":{followers}}}"#,
                a.shard_id.get(),
                a.slots.start,
                a.slots.end,
                leader,
            ));
        }
        assignments.push(']');
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"epoch":{},"slot_count":{},"assignments":{assignments}}}"#,
                map.epoch.get(),
                map.slot_count,
            ),
        ))
    }

    fn cluster_route(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let key = req
            .query
            .get("key")
            .cloned()
            .unwrap_or_else(|| "default".into());
        let consistency = req
            .query
            .get("consistency")
            .map(|s| parse_consistency(s))
            .or_else(|| req.header("x-ontolith-consistency").map(parse_consistency))
            .unwrap_or(ConsistencyLevel::Strong);
        let session = req
            .query
            .get("session")
            .cloned()
            .or_else(|| req.header("x-ontolith-session").map(|s| s.to_owned()));

        let write = self.cluster.route_write(&key)?;
        let read = if let Some(sid) = session {
            self.cluster
                .route_read_session(&key, &SessionId::new(sid), consistency)?
        } else {
            self.cluster.route_read(&key, consistency)?
        };
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"key":{},"write":{{"shard":{},"leader":{}}},"read":{{"shard":{},"target":{},"consistency":{},"served_by_leader":{},"max_staleness":{}}}}}"#,
                json_string(&key),
                write.shard_id.get(),
                json_string(write.leader_node.as_str()),
                read.shard_id.get(),
                json_string(read.target_node.as_str()),
                json_string(consistency.as_str()),
                read.served_by_leader,
                read.max_staleness_index
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "null".into()),
            ),
        ))
    }

    fn cluster_heartbeat(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        let node = req
            .query
            .get("node")
            .cloned()
            .or_else(|| req.header("x-ontolith-node").map(|s| s.to_owned()))
            .ok_or(OntolithError::InvalidArgument("missing node"))?;
        let tick = req
            .query
            .get("tick")
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| self.cluster_tick.load(Ordering::Relaxed));
        self.cluster
            .heartbeat(&ClusterNodeId::new(node.clone()), tick)?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"node":{},"tick":{}}}"#, json_string(&node), tick),
        ))
    }

    fn cluster_tick(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        let tick = req
            .query
            .get("tick")
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| self.cluster_tick.fetch_add(1, Ordering::Relaxed) + 1);
        self.cluster_tick.store(tick, Ordering::Relaxed);
        let events = self.cluster.tick(tick)?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"tick":{},"failovers":{},"status":{}}}"#,
                tick,
                events.len(),
                {
                    let st = self.cluster.status();
                    format!(
                        r#"{{"leader":{},"commit_index":{},"epoch":{}}}"#,
                        st.leader_id
                            .as_ref()
                            .map(|l| json_string(l.as_str()))
                            .unwrap_or_else(|| "null".into()),
                        st.commit_index,
                        st.epoch.get(),
                    )
                }
            ),
        ))
    }

    fn cluster_replicate(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        // Optional demo append
        if req
            .query
            .get("append")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false)
        {
            let _ = self
                .cluster
                .append(LogPayload::Metadata("api-append".into()))?;
        }
        let applied = self.cluster.replicate_to_followers()?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"applied_entries":{},"leader_index":{},"commit_index":{}}}"#,
                applied,
                self.cluster.leader_index(),
                self.cluster.commit_index(),
            ),
        ))
    }

    fn cluster_rebalance(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        let plans = self.cluster.rebalance()?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"plans":{},"epoch":{},"shards":{}}}"#,
                plans.len(),
                self.cluster.current_epoch().get(),
                self.cluster.shard_map().assignments.len(),
            ),
        ))
    }

    fn cluster_partition(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        let nodes = req
            .query
            .get("nodes")
            .map(|s| {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .map(|x| ClusterNodeId::new(x.trim()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if nodes.is_empty() {
            return Err(OntolithError::InvalidArgument(
                "partition requires ?nodes=n1,n2",
            ));
        }
        self.cluster.inject_partition(nodes.clone())?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(r#"{{"partitioned":{},"isolated":{}}}"#, nodes.len(), {
                let mut arr = String::from("[");
                for (i, n) in nodes.iter().enumerate() {
                    if i > 0 {
                        arr.push(',');
                    }
                    arr.push_str(&json_string(n.as_str()));
                }
                arr.push(']');
                arr
            }),
        ))
    }

    fn cluster_heal(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "cluster", "admin", now_ms())?;
        self.cluster.heal_partition()?;
        Ok(HttpResponse::json(200, "OK", r#"{"healed":true}"#))
    }

    fn cluster_failover_history(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let events = self.cluster.failover_history();
        let mut body = String::from("[");
        for (i, e) in events.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#"{{"tick":{},"shard":{},"old":{},"new":{},"reason":{}}}"#,
                e.at_tick,
                e.shard_id.get(),
                e.old_leader
                    .as_ref()
                    .map(|l| json_string(l.as_str()))
                    .unwrap_or_else(|| "null".into()),
                json_string(e.new_leader.as_str()),
                json_string(&e.reason),
            ));
        }
        body.push(']');
        Ok(HttpResponse::json(200, "OK", body))
    }
}

pub(crate) fn default_cluster() -> Arc<dyn ClusterRuntime> {
    let rt = Arc::new(InMemoryClusterRuntime::new(ClusterConfig {
        shard_count: 2,
        slot_count: 1024,
        ..Default::default()
    }));
    // Best-effort bootstrap; ignore if already initialized in tests that inject cluster.
    let _ = rt.bootstrap(vec![
        ("n1".into(), "127.0.0.1:7001".into()),
        ("n2".into(), "127.0.0.1:7002".into()),
        ("n3".into(), "127.0.0.1:7003".into()),
    ]);
    rt
}

/// Raft-backed cluster runtime selected from the environment (M3,
/// ADR-0004 decision 5). Env:
///   `ONTOLITH_CLUSTER_MODE=raft` (default in the management binary)
///   `ONTOLITH_RAFT_NODE_ID`    raft node id (0-based position in members)
///   `ONTOLITH_RAFT_LISTEN`     `ip:port` for the raft RPC server
///   `ONTOLITH_RAFT_SECRET`     shared cluster secret (HTTP transport)
///   `ONTOLITH_RAFT_STORAGE_PATH` optional RocksDB dir for the raft log
///   `ONTOLITH_RAFT_MEMBERS`    `n0=http://host:p0,n1=http://host:p1,...`
///
/// With no `ONTOLITH_RAFT_LISTEN` the in-memory transport is used and the
/// membership defaults to a single `n0=mem://n0` node.
#[cfg(feature = "raft-backend")]
pub(crate) fn default_raft_cluster() -> Result<Arc<dyn ClusterRuntime>, String> {
    use std::env;

    let node_id = env::var("ONTOLITH_RAFT_NODE_ID")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let listen = env::var("ONTOLITH_RAFT_LISTEN")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let secret = env::var("ONTOLITH_RAFT_SECRET").unwrap_or_default();
    let initial_slot_bias = env::var("ONTOLITH_RAFT_SLOT_BIAS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let storage_path = env::var("ONTOLITH_RAFT_STORAGE_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from);
    let members_env = env::var("ONTOLITH_RAFT_MEMBERS")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let http_transport = listen.is_some();

    if http_transport && secret.is_empty() {
        return Err(
            "raft HTTP transport requires a non-empty ONTOLITH_RAFT_SECRET".to_owned(),
        );
    }

    let rt = Arc::new(RaftClusterRuntime::new(RaftClusterConfig {
        node_id,
        http_listen_addr: listen,
        raft_secret: secret,
        raft_storage_path: storage_path,
        initial_slot_bias,
        ..RaftClusterConfig::default()
    }));

    let nodes: Vec<(String, String)> = match members_env {
        Some(list) => {
            let parsed = list
                .split(',')
                .map(|pair| {
                    let (id, addr) = pair
                        .split_once('=')
                        .ok_or_else(|| {
                            format!("invalid ONTOLITH_RAFT_MEMBERS entry '{pair}' (expected id=url)")
                        })?;
                    Ok((id.trim().to_owned(), addr.trim().to_owned()))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if parsed.is_empty() {
                return Err("ONTOLITH_RAFT_MEMBERS must not be empty".to_owned());
            }
            if node_id as usize >= parsed.len() {
                return Err(format!(
                    "ONTOLITH_RAFT_NODE_ID={node_id} is out of range for {} members",
                    parsed.len()
                ));
            }
            let http_members = parsed.iter().any(|(_, addr)| addr.starts_with("http://"));
            if http_members && !http_transport {
                return Err(
                    "ONTOLITH_RAFT_MEMBERS uses http:// addresses but ONTOLITH_RAFT_LISTEN is not set"
                        .to_owned(),
                );
            }
            if !http_members && http_transport {
                return Err(
                    "ONTOLITH_RAFT_LISTEN is set but ONTOLITH_RAFT_MEMBERS does not use http:// addresses"
                        .to_owned(),
                );
            }
            parsed
        }
        None => vec![("n0".into(), "mem://n0".into())],
    };

    rt.bootstrap(nodes)
        .map_err(|e| format!("raft cluster bootstrap: {}", e.message()))?;
    Ok(rt)
}

fn detect_ingest_format(req: &HttpRequest, path: &str) -> Result<ParseFormat, OntolithError> {
    if let Some(f) = req.query.get("format") {
        return parse_format_name(f);
    }
    if let Some(ct) = req.header("content-type") {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("trig") {
            return Ok(ParseFormat::TriG);
        }
        if ct.contains("turtle") || ct.contains("text/turtle") {
            return Ok(ParseFormat::Turtle);
        }
        if ct.contains("n-quads") || ct.contains("nquads") {
            return Ok(ParseFormat::NQuads);
        }
        if ct.contains("n-triples") || ct.contains("ntriples") {
            return Ok(ParseFormat::NTriples);
        }
    }
    Ok(match path {
        "/data/turtle" => ParseFormat::Turtle,
        "/data/trig" => ParseFormat::TriG,
        "/data/nq" => ParseFormat::NQuads,
        _ => ParseFormat::NTriples,
    })
}

fn parse_format_name(name: &str) -> Result<ParseFormat, OntolithError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "nt" | "ntriples" | "n-triples" => Ok(ParseFormat::NTriples),
        "nq" | "nquads" | "n-quads" => Ok(ParseFormat::NQuads),
        "ttl" | "turtle" => Ok(ParseFormat::Turtle),
        "trig" => Ok(ParseFormat::TriG),
        other => Err(OntolithError::Failed(format!(
            "unsupported ingest format: {other}"
        ))),
    }
}

/// Render the explain JSON shared by the HTTP and gRPC access paths (P5-01).
/// Result of the shared SPARQL execution path plus the reasoning execution
/// report when inference materialization ran (P6-03).
pub(crate) struct SparqlOutcome {
    pub result: QueryResult,
    pub reasoning: Option<ReasoningExecution>,
}

/// The inference mode used and the materializer's report (inferred triples,
/// elapsed, time-out, inconsistency flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReasoningExecution {
    pub mode: InferenceMode,
    pub report: ReasoningReport,
}

fn reasoning_meta(reasoning: Option<&ReasoningExecution>) -> String {
    match reasoning {
        None => String::new(),
        Some(execution) => format!(
            r#","reasoning":{{"mode":{},"inferred_triples":{},"elapsed_ms":{},"timed_out":{},"inconsistent":{}}}"#,
            json_string(execution.mode.as_str()),
            execution.report.inferred_triples,
            execution.report.elapsed_ms,
            execution.report.timed_out,
            execution.report.inconsistent,
        ),
    }
}

pub(crate) fn explain_json(
    plan: &QueryExplain,
    tenant: &str,
    consistency: ConsistencyLevel,
) -> String {
    format!(
        r#"{{"head":{{"plan_id":{},"kind":{}}},"algebra":{},"logical_steps":{},"physical_steps":{},"estimated_rows":{},"pattern_costs":{},"tenant":{},"consistency":{}}}"#,
        plan.plan_id.0,
        json_string(plan.kind.as_str()),
        json_string(&plan.algebra_summary),
        json_string_array(&plan.logical_steps),
        json_string_array(&plan.physical_steps),
        json_opt_number(plan.estimated_rows),
        json_pattern_costs(&plan.pattern_costs),
        json_string(tenant),
        json_string(consistency.as_str()),
    )
}

pub(crate) fn sparql_results_json(
    result: &QueryResult,
    ctx: &AuthContext,
    consistency: ConsistencyLevel,
    reasoning: Option<&ReasoningExecution>,
) -> String {
    let reasoning = reasoning_meta(reasoning);
    match result.kind {
        QueryKind::Ask => format!(
            r#"{{"head":{{}},"boolean":{},"meta":{{"elapsed_ms":{},"timed_out":{},"cancelled":{},"tenant":{},"consistency":{}{reasoning}}}}}"#,
            result.boolean.unwrap_or(false),
            result.elapsed_ms,
            result.timed_out,
            result.cancelled,
            json_string(ctx.tenant.as_str()),
            json_string(consistency.as_str()),
        ),
        QueryKind::Construct => {
            // Compact construct summary + sample triples.
            let mut triples = String::from("[");
            for (i, t) in result.construct_triples.iter().take(100).enumerate() {
                if i > 0 {
                    triples.push(',');
                }
                triples.push_str(&format!(
                    r#"{{"s":"n{}","p":{},"o":{}}}"#,
                    t.subject.get(),
                    json_string(t.predicate.as_str()),
                    term_json(&t.object)
                ));
            }
            triples.push(']');
            format!(
                r#"{{"head":{{"vars":[]}},"results":{{"triples":{triples},"count":{}}},"meta":{{"elapsed_ms":{},"timed_out":{},"tenant":{}{reasoning}}}}}"#,
                result.construct_triples.len(),
                result.elapsed_ms,
                result.timed_out,
                json_string(ctx.tenant.as_str()),
            )
        }
        QueryKind::Update => format!(
            r#"{{"head":{{}},"update":{{"affected":{}}},"meta":{{"elapsed_ms":{},"timed_out":{},"cancelled":{},"tenant":{},"consistency":{}{reasoning}}}}}"#,
            result.affected,
            result.elapsed_ms,
            result.timed_out,
            result.cancelled,
            json_string(ctx.tenant.as_str()),
            json_string(consistency.as_str()),
        ),
        _ => {
            // SELECT (and fallback): W3C SPARQL Results JSON-like
            let vars = json_string_array(&result.variables);
            let mut bindings = String::from("[");
            for (i, sol) in result.solutions.iter().enumerate() {
                if i > 0 {
                    bindings.push(',');
                }
                bindings.push('{');
                let mut first = true;
                for var in &result.variables {
                    if let Some(val) = sol.get(var) {
                        if !first {
                            bindings.push(',');
                        }
                        first = false;
                        bindings.push_str(&format!(
                            r#""{}":{}"#,
                            escape_json(var),
                            bound_value_json(val)
                        ));
                    }
                }
                // include unbound-less map: also dump any extra bindings not in variables
                if result.variables.is_empty() {
                    for (var, val) in &sol.bindings {
                        if !first {
                            bindings.push(',');
                        }
                        first = false;
                        bindings.push_str(&format!(
                            r#""{}":{}"#,
                            escape_json(var),
                            bound_value_json(val)
                        ));
                    }
                }
                bindings.push('}');
            }
            bindings.push(']');
            format!(
                r#"{{"head":{{"vars":{vars}}},"results":{{"bindings":{bindings}}},"meta":{{"row_count":{},"elapsed_ms":{},"timed_out":{},"cancelled":{},"tenant":{},"consistency":{}{reasoning}}}}}"#,
                result.row_count(),
                result.elapsed_ms,
                result.timed_out,
                result.cancelled,
                json_string(ctx.tenant.as_str()),
                json_string(consistency.as_str()),
            )
        }
    }
}

fn bound_value_json(val: &BoundValue) -> String {
    match val {
        BoundValue::Iri(iri) => {
            format!(r#"{{"type":"uri","value":{}}}"#, json_string(iri.as_str()))
        }
        BoundValue::Literal(lit) => {
            let s = lit.lexical_form();
            if let Some(lang) = lit.language_tag() {
                format!(
                    r#"{{"type":"literal","value":{},"xml:lang":{}}}"#,
                    json_string(&s),
                    json_string(lang.as_str())
                )
            } else if lit.xsd_datatype_iri().as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                format!(r#"{{"type":"literal","value":{}}}"#, json_string(&s))
            } else {
                format!(
                    r#"{{"type":"literal","value":{},"datatype":{}}}"#,
                    json_string(&s),
                    json_string(lit.xsd_datatype_iri().as_str())
                )
            }
        }
        BoundValue::Node(n) | BoundValue::Blank(n) => {
            format!(r#"{{"type":"bnode","value":"n{}"}}"#, n.get())
        }
    }
}

fn term_json(term: &ontolith_rdf::domain::Term) -> String {
    match term {
        ontolith_rdf::domain::Term::Iri(i) => json_string(i.as_str()),
        ontolith_rdf::domain::Term::BlankNode(n) => json_string(&format!("n{}", n.get())),
        ontolith_rdf::domain::Term::Literal(l) => json_string(&l.lexical_form()),
    }
}

fn extract_sparql_query(req: &HttpRequest) -> Result<String, OntolithError> {
    if let Some(q) = req.query.get("query") {
        return Ok(q.clone());
    }
    let ct = req.header("content-type").unwrap_or("");
    if ct.contains("application/sparql-query") || ct.contains("text/plain") {
        return Ok(req.body_str().to_owned());
    }
    if ct.contains("application/x-www-form-urlencoded") {
        for pair in req.body_str().split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "query"
            {
                return Ok(url_decode_form(v));
            }
        }
    }
    if !req.body.is_empty() {
        return Ok(req.body_str().to_owned());
    }
    Err(OntolithError::InvalidArgument("missing SPARQL query"))
}

fn url_decode_form(input: &str) -> String {
    let mut out = Vec::new();
    let b = input.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn parse_consistency(raw: &str) -> ConsistencyLevel {
    match raw.trim().to_ascii_lowercase().as_str() {
        "eventual" => ConsistencyLevel::Eventual,
        "session" => ConsistencyLevel::Session,
        _ => ConsistencyLevel::Strong,
    }
}

fn error_response(err: OntolithError) -> HttpResponse {
    let msg = err.message();
    let (status, reason) = if msg.starts_with("unauthorized") {
        (401, "Unauthorized")
    } else if msg.starts_with("forbidden") {
        (403, "Forbidden")
    } else if matches!(
        err,
        OntolithError::InvalidArgument(_) | OntolithError::InvalidState(_)
    ) {
        (400, "Bad Request")
    } else if matches!(err, OntolithError::Unsupported(_)) {
        (501, "Not Implemented")
    } else {
        (500, "Internal Server Error")
    };
    HttpResponse::json(
        status,
        reason,
        format!(
            r#"{{"error":{},"code":{}}}"#,
            json_string(msg),
            json_string(err.code())
        ),
    )
}

fn cors(mut resp: HttpResponse) -> HttpResponse {
    resp.headers
        .push(("Access-Control-Allow-Origin".into(), "*".into()));
    resp.headers.push((
        "Access-Control-Allow-Headers".into(),
        "Content-Type, Accept, X-API-Key, X-Ontolith-Tenant, X-Ontolith-User, X-Ontolith-Timeout-Ms, X-Ontolith-Explain, X-Ontolith-Consistency".into(),
    ));
    resp.headers.push((
        "Access-Control-Allow-Methods".into(),
        "GET, POST, OPTIONS".into(),
    ));
    resp
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", escape_json(s))
}

fn json_string_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(it));
    }
    out.push(']');
    out
}

fn json_opt_number(v: Option<u64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "null".into(),
    }
}

fn json_pattern_costs(costs: &[PatternCost]) -> String {
    let mut out = String::from("[");
    for (i, c) in costs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"pattern":{},"selectivity":{},"estimated_rows":{}}}"#,
            json_string(&c.pattern),
            c.selectivity,
            c.estimated_rows
        ));
    }
    out.push(']');
    out
}

pub fn shared_handler(state: Arc<AppState>) -> crate::http::Handler {
    Arc::new(move |req| state.handle(req))
}

pub fn dispatch_for_test(state: &Arc<AppState>, req: HttpRequest) -> HttpResponse {
    state.handle(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sparql_req(method: &str, query: &str) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_owned(),
            "application/sparql-query".to_owned(),
        );
        HttpRequest {
            method: method.to_owned(),
            path: "/sparql".to_owned(),
            query: HashMap::new(),
            headers,
            body: query.as_bytes().to_vec(),
        }
    }

    fn explain_req(method: &str, query: &str) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_owned(),
            "application/sparql-query".to_owned(),
        );
        HttpRequest {
            method: method.to_owned(),
            path: "/explain".to_owned(),
            query: HashMap::new(),
            headers,
            body: query.as_bytes().to_vec(),
        }
    }

    /// Authenticated request for enforced tenant-mode tests.
    fn tenant_req(
        method: &str,
        path: &str,
        query: HashMap<String, String>,
        body: &[u8],
        tenant: &str,
        user: &str,
    ) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert("x-ontolith-tenant".to_owned(), tenant.to_owned());
        headers.insert("x-ontolith-user".to_owned(), user.to_owned());
        headers.insert("x-api-key".to_owned(), "s3cret".to_owned());
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            query,
            headers,
            body: body.to_vec(),
        }
    }

    /// Enforced auth + enforced tenant isolation.
    fn enforced_tenant_state() -> Arc<AppState> {
        AppState::new_memory_with_audit(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator {
                mode: AuthMode::Enforced,
                api_key: Some("s3cret".to_owned()),
                ..HeaderAuthenticator::default()
            },
            InMemoryAuditLog::new(),
            TenantMode::Enforced,
        )
    }

    fn memory_state_with_inference(inference: InferenceConfig) -> Arc<AppState> {
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let dictionary: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        AppState::from_parts(
            storage,
            dictionary,
            triples,
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator::default(),
            StorageBackendKind::Memory,
            None,
            default_cluster(),
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
            inference,
        )
    }

    fn enforced_tenant_state_with_inference(inference: InferenceConfig) -> Arc<AppState> {
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let dictionary: Arc<dyn DictionaryCodec> = Arc::new(InMemoryDictionary::new());
        let triples: Arc<dyn TripleRepository> =
            Arc::new(EngineTripleRepository::new(Arc::clone(&storage)));
        AppState::from_parts(
            storage,
            dictionary,
            triples,
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator {
                mode: AuthMode::Enforced,
                api_key: Some("s3cret".to_owned()),
                ..HeaderAuthenticator::default()
            },
            StorageBackendKind::Memory,
            None,
            default_cluster(),
            InMemoryAuditLog::new(),
            TenantMode::Enforced,
            inference,
        )
    }

    fn sparql_req_with(method: &str, query: &str, params: HashMap<String, String>) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_owned(),
            "application/sparql-query".to_owned(),
        );
        HttpRequest {
            method: method.to_owned(),
            path: "/sparql".to_owned(),
            query: params,
            headers,
            body: query.as_bytes().to_vec(),
        }
    }

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
    const OWL_DISJOINT: &str = "http://www.w3.org/2002/07/owl#disjointWith";

    #[test]
    fn enforced_tenant_mode_isolates_reads_and_writes() {
        let state = enforced_tenant_state();

        // Health surfaces the tenant posture.
        let health = dispatch_for_test(
            &state,
            tenant_req(
                "GET",
                "/health",
                HashMap::new(),
                b"",
                "acme",
                "u1",
            ),
        );
        assert_eq!(health.status, 200);
        assert!(String::from_utf8_lossy(&health.body).contains("\"tenant_mode\":\"enforced\""));

        // Tenant acme writes; the data is stamped into its tenant graph.
        let write = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"INSERT DATA { <http://ex.org/s1> <http://ex.org/p> \"acme-data\" }",
                "acme",
                "u1",
            ),
        );
        assert_eq!(
            write.status,
            200,
            "body={}",
            String::from_utf8_lossy(&write.body)
        );

        // Tenant other writes its own data.
        let write = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"INSERT DATA { <http://ex.org/s2> <http://ex.org/p> \"other-data\" }",
                "other",
                "u2",
            ),
        );
        assert_eq!(write.status, 200);

        // Each tenant's default graph is its own namespace.
        let acme_read = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"SELECT ?s ?o WHERE { ?s <http://ex.org/p> ?o }",
                "acme",
                "u1",
            ),
        );
        let body = String::from_utf8_lossy(&acme_read.body);
        assert!(body.contains("acme-data"), "acme read: {body}");
        assert!(!body.contains("other-data"), "acme must not see other tenant: {body}");

        let other_read = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"SELECT ?s ?o WHERE { ?s <http://ex.org/p> ?o }",
                "other",
                "u2",
            ),
        );
        let body = String::from_utf8_lossy(&other_read.body);
        assert!(body.contains("other-data"), "other read: {body}");
        assert!(!body.contains("acme-data"), "other must not see acme tenant: {body}");
    }

    #[test]
    fn enforced_tenant_mode_rejects_cross_tenant_references() {
        let state = enforced_tenant_state();

        // Explicit graph write outside the tenant namespace -> 403.
        let mut query = HashMap::new();
        query.insert("graph".to_owned(), "urn:tenant:other".to_owned());
        let resp = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/data/nt",
                query,
                b"<http://ex.org/s> <http://ex.org/p> \"o\" .",
                "acme",
                "u1",
            ),
        );
        assert_eq!(resp.status, 403, "body={}", String::from_utf8_lossy(&resp.body));

        // Named-graph quad in the payload outside the namespace -> 403.
        let resp = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/data/trig",
                HashMap::new(),
                b"<urn:tenant:other> { <http://ex.org/s> <http://ex.org/p> \"o\" }",
                "acme",
                "u1",
            ),
        );
        assert_eq!(resp.status, 403, "body={}", String::from_utf8_lossy(&resp.body));

        // SPARQL GRAPH reference to a foreign tenant -> 403.
        let resp = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"SELECT * WHERE { GRAPH <urn:tenant:other> { ?s ?p ?o } }",
                "acme",
                "u1",
            ),
        );
        assert_eq!(resp.status, 403, "body={}", String::from_utf8_lossy(&resp.body));

        // SPARQL FROM reference to a foreign tenant -> 403.
        let resp = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                b"SELECT * FROM <urn:tenant:other> WHERE { ?s ?p ?o }",
                "acme",
                "u1",
            ),
        );
        assert_eq!(resp.status, 403, "body={}", String::from_utf8_lossy(&resp.body));

        // An owned sub-graph write succeeds.
        let mut query = HashMap::new();
        query.insert("graph".to_owned(), "urn:tenant:acme:sub".to_owned());
        let resp = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/data/nt",
                query,
                b"<http://ex.org/s> <http://ex.org/p> \"o\" .",
                "acme",
                "u1",
            ),
        );
        assert_eq!(resp.status, 200, "body={}", String::from_utf8_lossy(&resp.body));
        assert!(String::from_utf8_lossy(&resp.body).contains("urn:tenant:acme:sub"));
    }

    fn jwt_state() -> Arc<AppState> {
        AppState::new_memory_with_audit(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator {
                mode: AuthMode::Enforced,
                jwt_secret: Some("s3cret".to_owned()),
                jwt_issuer: Some("ontolith".to_owned()),
                jwt_audience: Some("ontolith-server".to_owned()),
                ..HeaderAuthenticator::default()
            },
            InMemoryAuditLog::new(),
            TenantMode::Enforced,
        )
    }

    fn jwt_req(method: &str, path: &str, body: &[u8], token: &str) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert("authorization".to_owned(), format!("Bearer {token}"));
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            query: HashMap::new(),
            headers,
            body: body.to_vec(),
        }
    }

    #[test]
    fn jwt_bearer_token_authenticates_requests() {
        use ontolith_security::infrastructure::sign_tenant_token;
        let state = jwt_state();
        let token =
            sign_tenant_token("acme", "alice", "s3cret", "ontolith", "ontolith-server", 300)
                .unwrap();

        // The bearer token alone authenticates (no header credentials).
        let resp = dispatch_for_test(&state, jwt_req("GET", "/health", b"", &token));
        assert_eq!(resp.status, 200, "body={}", String::from_utf8_lossy(&resp.body));
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("\"auth_mode\":\"enforced\""), "body={body}");
        assert!(body.contains("\"jwt\":\"on\""), "body={body}");
    }

    #[test]
    fn jwt_rejects_forged_expired_and_misaligned_tokens() {
        use ontolith_security::infrastructure::sign_tenant_token;
        let state = jwt_state();

        // Forged secret and expired tokens are rejected.
        let forged =
            sign_tenant_token("acme", "alice", "wrong", "ontolith", "ontolith-server", 300)
                .unwrap();
        let expired =
            sign_tenant_token("acme", "alice", "s3cret", "ontolith", "ontolith-server", -10)
                .unwrap();
        for token in [forged, expired] {
            let resp = dispatch_for_test(&state, jwt_req("GET", "/health", b"", &token));
            assert_eq!(resp.status, 401, "body={}", String::from_utf8_lossy(&resp.body));
        }

        // JWT tenant claim wins over the transport header: a write issued
        // with a conflicting header tenant is stamped into the JWT tenant.
        let token =
            sign_tenant_token("acme", "alice", "s3cret", "ontolith", "ontolith-server", 300)
                .unwrap();
        let mut headers = HashMap::new();
        headers.insert("authorization".to_owned(), format!("Bearer {token}"));
        headers.insert("x-ontolith-tenant".to_owned(), "other".to_owned());
        headers.insert("x-ontolith-user".to_owned(), "mallory".to_owned());
        let resp = dispatch_for_test(
            &state,
            HttpRequest {
                method: "POST".to_owned(),
                path: "/data/nt".to_owned(),
                query: HashMap::new(),
                headers,
                body: b"<http://ex.org/j1> <http://ex.org/p> \"jwt-tenant-wins\" .".to_vec(),
            },
        );
        assert_eq!(resp.status, 200, "body={}", String::from_utf8_lossy(&resp.body));

        let read = dispatch_for_test(
            &state,
            jwt_req(
                "POST",
                "/sparql",
                b"SELECT ?s ?o WHERE { ?s <http://ex.org/p> ?o }",
                &token,
            ),
        );
        let body = String::from_utf8_lossy(&read.body);
        assert!(body.contains("jwt-tenant-wins"), "acme read: {body}");
    }

    #[test]
    fn tracing_records_full_chain_spans() {
        use ontolith_observability::infrastructure::{
            format_traceparent, generate_span_id, generate_trace_id,
        };
        let state = AppState::new_memory(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator::default(),
        );
        let upstream_trace = generate_trace_id();
        let upstream_span = generate_span_id();

        let mut req = sparql_req("POST", "SELECT * WHERE { ?s ?p ?o } LIMIT 1");
        req.headers.insert(
            "traceparent".to_owned(),
            format_traceparent(&upstream_trace, &upstream_span),
        );
        let resp = dispatch_for_test(&state, req);
        assert_eq!(resp.status, 200, "body={}", String::from_utf8_lossy(&resp.body));

        // The response echoes the same trace id for downstream propagation.
        let echoed = resp
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("traceparent"))
            .expect("response must carry a traceparent header");
        assert!(echoed.1.starts_with(&format!("00-{}", upstream_trace.0)));

        // Full chain: http.request (root) -> http.auth + sparql.execute.
        let spans = state.traces.spans();
        let names: Vec<&str> = spans.iter().map(|s| s.name.0.as_str()).collect();
        assert!(names.contains(&"http.request"), "spans={names:?}");
        assert!(names.contains(&"http.auth"), "spans={names:?}");
        assert!(names.contains(&"sparql.execute"), "spans={names:?}");

        let root = spans
            .iter()
            .find(|s| s.name.0 == "http.request")
            .expect("root span");
        assert_eq!(root.parent_span_id.as_ref().unwrap().0, upstream_span.0);
        let query_span = spans
            .iter()
            .find(|s| s.name.0 == "sparql.execute")
            .expect("query span");
        assert_eq!(
            query_span.parent_span_id.as_ref().unwrap().0,
            root.span_id.0
        );
        assert!(query_span
            .attributes
            .iter()
            .any(|(k, v)| k == "tenant" && v == "system"));
        assert_eq!(root.status, SpanStatus::Ok);
    }

    #[test]
    fn tracing_records_error_spans_for_failed_auth() {
        let state = AppState::new_memory(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator {
                mode: AuthMode::Enforced,
                api_key: Some("s3cret".to_owned()),
                ..HeaderAuthenticator::default()
            },
        );
        let resp = dispatch_for_test(
            &state,
            HttpRequest {
                method: "GET".to_owned(),
                path: "/health".to_owned(),
                query: HashMap::new(),
                headers: HashMap::new(),
                body: Vec::new(),
            },
        );
        assert_eq!(resp.status, 401);

        let spans = state.traces.spans();
        let root = spans
            .iter()
            .find(|s| s.name.0 == "http.request")
            .expect("root span");
        assert_eq!(root.status, SpanStatus::Error);
        let auth_span = spans
            .iter()
            .find(|s| s.name.0 == "http.auth")
            .expect("auth span");
        assert_eq!(auth_span.status, SpanStatus::Error);
        assert_eq!(
            auth_span.parent_span_id.as_ref().unwrap().0,
            root.span_id.0
        );
    }

    #[test]
    fn sparql_update_insert_data_via_http() {
        let state =
            AppState::new_memory("127.0.0.1:8080".to_owned(), HeaderAuthenticator::default());
        let resp = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                "INSERT DATA { <http://ex.org/a> <http://ex.org/b> \"c\" }",
            ),
        );
        assert_eq!(
            resp.status,
            200,
            "body={}",
            String::from_utf8_lossy(&resp.body)
        );
        assert!(String::from_utf8_lossy(&resp.body).contains("\"affected\":1"));

        let read = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s <http://ex.org/b> ?o }",
            ),
        );
        assert_eq!(read.status, 200);
        assert!(String::from_utf8_lossy(&read.body).contains("\"c\""));
    }

    #[test]
    fn explain_via_http_includes_cost_estimates() {
        let state =
            AppState::new_memory("127.0.0.1:8080".to_owned(), HeaderAuthenticator::default());
        let insert = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                "INSERT DATA { <http://ex.org/a> <http://ex.org/b> \"c\" }",
            ),
        );
        assert_eq!(insert.status, 200);

        let resp = dispatch_for_test(
            &state,
            explain_req("POST", "SELECT * WHERE { ?s <http://ex.org/b> ?o }"),
        );
        assert_eq!(
            resp.status,
            200,
            "body={}",
            String::from_utf8_lossy(&resp.body)
        );
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("\"estimated_rows\":1"), "body={body}");
        assert!(body.contains("\"pattern_costs\":["), "body={body}");
        assert!(
            body.contains("\"pattern\":\"?s <http://ex.org/b> ?o\""),
            "body={body}"
        );
    }

    #[test]
    fn inference_forward_chain_materializes_query_results() {
        let state = memory_state_with_inference(InferenceConfig::new(
            InferenceMode::ForwardChaining,
            64,
            None,
        ));
        let insert = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!(
                    "INSERT DATA {{ <http://ex.org/A> <{RDFS_SUBCLASS}> <http://ex.org/B> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/A> }}"
                ),
            ),
        );
        assert_eq!(
            insert.status,
            200,
            "body={}",
            String::from_utf8_lossy(&insert.body)
        );

        let query = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/B> }}"),
            ),
        );
        assert_eq!(
            query.status,
            200,
            "body={}",
            String::from_utf8_lossy(&query.body)
        );
        let body = String::from_utf8_lossy(&query.body);
        assert!(
            body.contains("\"row_count\":1"),
            "inferred typing must be visible (row_count 1): {body}"
        );
        assert!(body.contains("\"s\":"), "subject binding present: {body}");
        assert!(body.contains("\"reasoning\""), "reasoning meta: {body}");
        assert!(body.contains("\"mode\":\"forward\""), "mode: {body}");
        assert!(
            body.contains("\"inferred_triples\":1"),
            "inferred count: {body}"
        );
    }

    #[test]
    fn inference_default_off_and_query_param_override() {
        let state =
            AppState::new_memory("127.0.0.1:8080".to_owned(), HeaderAuthenticator::default());
        let insert = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!(
                    "INSERT DATA {{ <http://ex.org/A> <{RDFS_SUBCLASS}> <http://ex.org/B> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/A> }}"
                ),
            ),
        );
        assert_eq!(insert.status, 200);

        // Default (off): the subClassOf typing is not materialized.
        let base = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/B> }}"),
            ),
        );
        let base_body = String::from_utf8_lossy(&base.body);
        assert!(
            !base_body.contains("\"row_count\":1"),
            "off must not infer: {base_body}"
        );
        assert!(
            !base_body.contains("\"reasoning\""),
            "off must not emit reasoning meta: {base_body}"
        );

        // Per-request override turns reasoning on for one query.
        let mut params = HashMap::new();
        params.insert("inference".to_owned(), "forward".to_owned());
        let overridden = dispatch_for_test(
            &state,
            sparql_req_with(
                "POST",
                &format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/B> }}"),
                params,
            ),
        );
        let overridden_body = String::from_utf8_lossy(&overridden.body);
        assert_eq!(overridden.status, 200, "body={overridden_body}");
        assert!(
            overridden_body.contains("\"row_count\":1"),
            "override forward must infer (row_count 1): {overridden_body}"
        );
        assert!(
            overridden_body.contains("\"mode\":\"forward\""),
            "override meta: {overridden_body}"
        );

        // Invalid override is rejected.
        let mut params = HashMap::new();
        params.insert("inference".to_owned(), "bogus".to_owned());
        let invalid = dispatch_for_test(
            &state,
            sparql_req_with(
                "POST",
                "SELECT ?s WHERE { ?s ?p ?o }",
                params,
            ),
        );
        assert_eq!(invalid.status, 400, "invalid override must 400");
    }

    #[test]
    fn inference_reports_inconsistent_ontology() {
        let state = memory_state_with_inference(InferenceConfig::new(
            InferenceMode::ForwardChaining,
            64,
            None,
        ));
        let insert = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!(
                    "INSERT DATA {{ <http://ex.org/A> <{OWL_DISJOINT}> <http://ex.org/B> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/A> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/B> }}"
                ),
            ),
        );
        assert_eq!(
            insert.status,
            200,
            "body={}",
            String::from_utf8_lossy(&insert.body)
        );

        let query = dispatch_for_test(
            &state,
            sparql_req("POST", "SELECT ?s WHERE { ?s ?p ?o }"),
        );
        assert_eq!(query.status, 200);
        let body = String::from_utf8_lossy(&query.body);
        assert!(
            body.contains("\"inconsistent\":true"),
            "inconsistency must be surfaced: {body}"
        );
    }

    #[test]
    fn inference_elapsed_guard_marks_timed_out() {
        let state = memory_state_with_inference(InferenceConfig::new(
            InferenceMode::ForwardChaining,
            64,
            Some(0),
        ));
        let insert = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!(
                    "INSERT DATA {{ <http://ex.org/A> <{RDFS_SUBCLASS}> <http://ex.org/B> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/A> }}"
                ),
            ),
        );
        assert_eq!(insert.status, 200);

        let query = dispatch_for_test(
            &state,
            sparql_req(
                "POST",
                &format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/B> }}"),
            ),
        );
        assert_eq!(query.status, 200);
        let body = String::from_utf8_lossy(&query.body);
        assert!(
            body.contains("\"reasoning\"") && body.contains("\"timed_out\":true"),
            "elapsed guard must mark timed_out: {body}"
        );
    }

    #[test]
    fn inference_respects_tenant_isolation() {
        let state = enforced_tenant_state_with_inference(InferenceConfig::new(
            InferenceMode::ForwardChaining,
            64,
            None,
        ));
        let acme_write = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                format!(
                    "INSERT DATA {{ <http://ex.org/A> <{RDFS_SUBCLASS}> <http://ex.org/B> . <http://ex.org/x> <{RDF_TYPE}> <http://ex.org/A> }}"
                )
                .as_bytes(),
                "acme",
                "u1",
            ),
        );
        assert_eq!(acme_write.status, 200);
        let other_write = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                format!(
                    "INSERT DATA {{ <http://ex.org/C> <{RDFS_SUBCLASS}> <http://ex.org/D> . <http://ex.org/y> <{RDF_TYPE}> <http://ex.org/C> }}"
                )
                .as_bytes(),
                "other",
                "u2",
            ),
        );
        assert_eq!(other_write.status, 200);

        // acme's inference sees only its own closure: B-typing for x, never D.
        let acme_read = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/B> }}").as_bytes(),
                "acme",
                "u1",
            ),
        );
        let acme_body = String::from_utf8_lossy(&acme_read.body);
        assert!(
            acme_body.contains("\"row_count\":1"),
            "acme must see its inferred B typing (row_count 1): {acme_body}"
        );
        let acme_foreign = dispatch_for_test(
            &state,
            tenant_req(
                "POST",
                "/sparql",
                HashMap::new(),
                format!("SELECT ?s WHERE {{ ?s <{RDF_TYPE}> <http://ex.org/D> }}").as_bytes(),
                "acme",
                "u1",
            ),
        );
        let foreign_body = String::from_utf8_lossy(&acme_foreign.body);
        assert!(
            foreign_body.contains("\"row_count\":0"),
            "acme must not see other tenant's inferred D typing: {foreign_body}"
        );
    }
}
