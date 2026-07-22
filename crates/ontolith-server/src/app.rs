//! Application state and route handlers for L5 HTTP gateway.

use crate::http::{HttpRequest, HttpResponse, now_ms};
use ontolith_cluster::application::{
    ClusterRuntime, FailoverController, FaultInjector, MetadataService, RebalanceService,
    Replicator, ShardRouter,
};
use ontolith_cluster::domain::{ClusterNodeId, LogPayload, SessionId};
use ontolith_cluster::infrastructure::{ClusterConfig, InMemoryClusterRuntime};
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;
use ontolith_observability::domain::{MetricKind, MetricPoint};
use ontolith_observability::infrastructure::{InMemoryMetricSink, render_prometheus_text};
use ontolith_parser::domain::ParseFormat;
use ontolith_parser::infrastructure::{
    parse_nquads, parse_ntriples, parse_trig_doc, parse_turtle_doc,
};
use ontolith_query::domain::{BoundValue, QueryKind, QueryRequest, QueryResult};
use ontolith_query::infrastructure::standard_pipeline;
use ontolith_security::application::{
    Authenticator, HeaderAuthenticator, InMemoryAuditLog, authorize,
};
use ontolith_security::domain::{AuditOutcome, AuthContext, AuthMode};
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
    pub cluster: Arc<InMemoryClusterRuntime>,
    pub cluster_tick: AtomicU64,
}

impl AppState {
    pub fn new_memory(bind_address: String, auth: HeaderAuthenticator) -> Arc<Self> {
        Self::new_memory_with_audit(bind_address, auth, InMemoryAuditLog::new())
    }

    pub fn new_memory_with_audit(
        bind_address: String,
        auth: HeaderAuthenticator,
        audit: InMemoryAuditLog,
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
        )
    }

    #[cfg(feature = "rocksdb-backend")]
    pub fn new_rocksdb(
        bind_address: String,
        auth: HeaderAuthenticator,
        path: PathBuf,
    ) -> Result<Arc<Self>, OntolithError> {
        Self::new_rocksdb_with_audit(bind_address, auth, path, InMemoryAuditLog::new())
    }

    #[cfg(feature = "rocksdb-backend")]
    pub fn new_rocksdb_with_audit(
        bind_address: String,
        auth: HeaderAuthenticator,
        path: PathBuf,
        audit: InMemoryAuditLog,
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
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        storage: Arc<dyn StorageEngine>,
        dictionary: Arc<dyn DictionaryCodec>,
        triples: Arc<dyn TripleRepository>,
        bind_address: String,
        auth: HeaderAuthenticator,
        backend: StorageBackendKind,
        data_dir: Option<PathBuf>,
        cluster: Arc<InMemoryClusterRuntime>,
        audit: InMemoryAuditLog,
    ) -> Arc<Self> {
        Arc::new(Self {
            storage,
            dictionary,
            triples,
            txns: Arc::new(InMemoryTransactionManager::new()),
            authenticator: auth,
            audit,
            metrics: InMemoryMetricSink::new(),
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
        self.authenticator.authenticate(
            req.header("x-ontolith-tenant"),
            req.header("x-ontolith-user"),
            req.header("x-api-key"),
        )
    }

    fn health(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "health", "read", now_ms())?;
        let stats = self.storage.stats();
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"status":"ok","layer":"L5","bind":{},"backend":{},"triples":{},"quads":{},"pending_txns":{},"auth_mode":{},"data_dir":{}}}"#,
                json_string(&self.bind_address),
                json_string(self.backend.as_str()),
                stats.triple_count,
                stats.quad_count,
                stats.pending_transactions,
                json_string(match self.authenticator.mode {
                    AuthMode::Disabled => "disabled",
                    AuthMode::Enforced => "enforced",
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

        let mut qreq = QueryRequest::new(query_text.clone()).with_consistency(consistency);
        qreq.tenant = Some(ctx.tenant.as_str().to_owned());
        if let Some(t) = timeout_ms {
            qreq = qreq.with_timeout(t);
        }

        let pipeline = standard_pipeline(Arc::clone(&self.triples));

        if explain {
            let plan = pipeline.explain(&qreq)?;
            let body = format!(
                r#"{{"head":{{"plan_id":{},"kind":{}}},"algebra":{},"logical_steps":{},"physical_steps":{},"tenant":{},"consistency":{}}}"#,
                plan.plan_id.0,
                json_string(plan.kind.as_str()),
                json_string(&plan.algebra_summary),
                json_string_array(&plan.logical_steps),
                json_string_array(&plan.physical_steps),
                json_string(ctx.tenant.as_str()),
                json_string(consistency.as_str()),
            );
            self.audit.record(
                now_ms(),
                &ctx,
                "explain",
                "sparql",
                AuditOutcome::Allow,
                format!("plan={}", plan.plan_id.0),
            );
            return Ok(HttpResponse::json(200, "OK", body));
        }

        let result = pipeline.execute(&qreq)?;
        self.audit.record(
            now_ms(),
            &ctx,
            "query",
            "sparql",
            AuditOutcome::Allow,
            format!("rows={}", result.row_count()),
        );

        // SPARQL Query Results JSON Format (W3C-inspired) when accept/format asks for it.
        if format.contains("sparql-results") || format == "srj" || format == "json" {
            return Ok(HttpResponse::json(
                200,
                "OK",
                sparql_results_json(&result, &ctx, consistency),
            ));
        }
        Ok(HttpResponse::json(
            200,
            "OK",
            sparql_results_json(&result, &ctx, consistency),
        ))
    }

    fn ingest(&self, req: &HttpRequest, path: &str) -> Result<HttpResponse, OntolithError> {
        let ctx = self.auth(req)?;
        authorize(&self.audit, &ctx, "data", "write", now_ms())?;
        self.ingest_total.fetch_add(1, Ordering::Relaxed);

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

        // Tenant isolation at write path: stamp graph name with tenant if requested.
        let tenant_graph = req.query.get("graph").cloned().or_else(|| {
            if req
                .query
                .get("tenant_graph")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false)
            {
                Some(format!("urn:tenant:{}", ctx.tenant.as_str()))
            } else {
                None
            }
        });

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

fn default_cluster() -> Arc<InMemoryClusterRuntime> {
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

fn sparql_results_json(
    result: &QueryResult,
    ctx: &AuthContext,
    consistency: ConsistencyLevel,
) -> String {
    match result.kind {
        QueryKind::Ask => format!(
            r#"{{"head":{{}},"boolean":{},"meta":{{"elapsed_ms":{},"timed_out":{},"cancelled":{},"tenant":{},"consistency":{}}}}}"#,
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
                r#"{{"head":{{"vars":[]}},"results":{{"triples":{triples},"count":{}}},"meta":{{"elapsed_ms":{},"timed_out":{},"tenant":{}}}}}"#,
                result.construct_triples.len(),
                result.elapsed_ms,
                result.timed_out,
                json_string(ctx.tenant.as_str()),
            )
        }
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
                r#"{{"head":{{"vars":{vars}}},"results":{{"bindings":{bindings}}},"meta":{{"row_count":{},"elapsed_ms":{},"timed_out":{},"cancelled":{},"tenant":{},"consistency":{}}}}}"#,
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
            let s = match lit {
                ontolith_core::domain::LiteralValue::String(s) => s.clone(),
                ontolith_core::domain::LiteralValue::Integer(v) => v.to_string(),
                ontolith_core::domain::LiteralValue::Decimal(v) => v.to_string(),
                ontolith_core::domain::LiteralValue::Boolean(v) => v.to_string(),
            };
            let dt = match lit {
                ontolith_core::domain::LiteralValue::String(_) => None,
                ontolith_core::domain::LiteralValue::Integer(_) => {
                    Some("http://www.w3.org/2001/XMLSchema#integer")
                }
                ontolith_core::domain::LiteralValue::Decimal(_) => {
                    Some("http://www.w3.org/2001/XMLSchema#double")
                }
                ontolith_core::domain::LiteralValue::Boolean(_) => {
                    Some("http://www.w3.org/2001/XMLSchema#boolean")
                }
            };
            match dt {
                Some(d) => format!(
                    r#"{{"type":"literal","value":{},"datatype":{}}}"#,
                    json_string(&s),
                    json_string(d)
                ),
                None => format!(r#"{{"type":"literal","value":{}}}"#, json_string(&s)),
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
        ontolith_rdf::domain::Term::Literal(l) => {
            let s = match l {
                ontolith_core::domain::LiteralValue::String(s) => s.clone(),
                ontolith_core::domain::LiteralValue::Integer(v) => v.to_string(),
                ontolith_core::domain::LiteralValue::Decimal(v) => v.to_string(),
                ontolith_core::domain::LiteralValue::Boolean(v) => v.to_string(),
            };
            json_string(&s)
        }
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

fn parse_consistency(raw: &str) -> ConsistencyLevel {
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

pub fn shared_handler(state: Arc<AppState>) -> crate::http::Handler {
    Arc::new(move |req| state.handle(req))
}

pub fn dispatch_for_test(state: &Arc<AppState>, req: HttpRequest) -> HttpResponse {
    state.handle(req)
}
