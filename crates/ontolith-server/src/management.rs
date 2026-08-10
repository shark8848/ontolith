//! Management server for unified control-plane operations.

use crate::app::AppState;
use crate::http::{Handler, HttpRequest, HttpResponse, HttpServer, TlsServerConfig, now_ms};
use crate::reasoning::InferenceConfig;
use ontolith_cluster::domain::LogPayload;
use ontolith_core::error::OntolithError;
use ontolith_observability::infrastructure::render_traces_json;
use ontolith_plugin_api::domain::{
    PluginCapability, PluginId, PluginManifest, ToolDefinition, ToolParam,
};
use ontolith_security::application::{
    Authenticator, HeaderAuthenticator, InMemoryAuditLog, authorize,
};
use ontolith_security::domain::{AuditOutcome, AuthContext, AuthMode, TenantMode};
use ontolith_security::infrastructure::{
    CachingJwks, FileAuditLog, Jwks, JwksFetcher, JwksVerifier,
};
use std::env;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const MGMT_BIND_ENV: &str = "ONTOLITH_MANAGEMENT_BIND";
const API_BIND_ENV: &str = "ONTOLITH_BIND";
const STORAGE_ENV: &str = "ONTOLITH_STORAGE";
const DATA_DIR_ENV: &str = "ONTOLITH_DATA_DIR";
const AUTH_MODE_ENV: &str = "ONTOLITH_AUTH_MODE";
const TENANT_MODE_ENV: &str = "ONTOLITH_TENANT_MODE";
const API_KEY_ENV: &str = "ONTOLITH_API_KEY";
const JWT_SECRET_ENV: &str = "ONTOLITH_JWT_SECRET";
const JWT_ISSUER_ENV: &str = "ONTOLITH_JWT_ISSUER";
const JWT_AUDIENCE_ENV: &str = "ONTOLITH_JWT_AUDIENCE";
const JWT_LEEWAY_ENV: &str = "ONTOLITH_JWT_LEEWAY_SECS";
const OIDC_ISSUER_ENV: &str = "ONTOLITH_OIDC_ISSUER";
const OIDC_AUDIENCE_ENV: &str = "ONTOLITH_OIDC_AUDIENCE";
const OIDC_JWKS_URL_ENV: &str = "ONTOLITH_OIDC_JWKS_URL";
const OIDC_CACHE_TTL_SECS_ENV: &str = "ONTOLITH_OIDC_CACHE_TTL_SECS";
const AUDIT_PATH_ENV: &str = "ONTOLITH_AUDIT_PATH";
const CLUSTER_MODE_ENV: &str = "ONTOLITH_CLUSTER_MODE";
const MGMT_READ_KEY_ENV: &str = "ONTOLITH_MANAGEMENT_READ_KEY";
const MGMT_WRITE_KEY_ENV: &str = "ONTOLITH_MANAGEMENT_WRITE_KEY";
const MGMT_KEY_HEADER: &str = "x-ontolith-management-key";
const MGMT_RUNTIME_PROBE_TIMEOUT_MS_ENV: &str = "ONTOLITH_MANAGEMENT_PROBE_TIMEOUT_MS";
const TLS_CERT_ENV: &str = "ONTOLITH_TLS_CERT";
const TLS_KEY_ENV: &str = "ONTOLITH_TLS_KEY";

const DEFAULT_MGMT_BIND: &str = "127.0.0.1:9091";
const DEFAULT_API_BIND: &str = "127.0.0.1:8080";

pub struct ManagementState {
    app: Arc<AppState>,
    management_bind: String,
    started_at_ms: u64,
    acl: ManagementAcl,
    runtime_probe_timeout_ms: u64,
    tls_enabled: bool,
}

#[derive(Debug, Clone, Default)]
struct ManagementAcl {
    read_key: Option<String>,
    write_key: Option<String>,
}

impl ManagementAcl {
    fn enabled(&self) -> bool {
        self.read_key.is_some() || self.write_key.is_some()
    }

    fn allows_read(&self, provided: Option<&str>) -> bool {
        if !self.enabled() {
            return true;
        }

        match provided {
            Some(value) => {
                self.read_key.as_deref() == Some(value) || self.write_key.as_deref() == Some(value)
            }
            None => false,
        }
    }

    fn allows_write(&self, provided: Option<&str>) -> bool {
        if !self.enabled() {
            return true;
        }

        match (&self.write_key, &self.read_key, provided) {
            (Some(write), _, Some(value)) => write == value,
            (None, Some(read), Some(value)) => read == value,
            _ => false,
        }
    }
}

impl ManagementState {
    fn new(
        app: Arc<AppState>,
        management_bind: String,
        acl: ManagementAcl,
        runtime_probe_timeout_ms: u64,
        tls_enabled: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            management_bind,
            started_at_ms: now_ms(),
            acl,
            runtime_probe_timeout_ms,
            tls_enabled,
        })
    }

    pub fn handle(self: &Arc<Self>, req: HttpRequest) -> HttpResponse {
        let method = req.method.to_ascii_uppercase();
        let path = req.path.as_str();

        if method == "OPTIONS" {
            return cors(HttpResponse::text(204, "No Content", ""));
        }

        let result = match (method.as_str(), path) {
            ("GET", "/health") | ("GET", "/healthz") | ("GET", "/admin/health") => {
                self.admin_health(&req)
            }
            ("GET", "/admin/config") => self.admin_config(&req),
            ("GET", "/admin/layers") => self.admin_layers(&req),
            ("GET", "/admin/plugins") => self.admin_plugins(&req),
            ("GET", "/admin/monitoring") => self.admin_monitoring(&req),
            ("GET", "/admin/traces") => self.admin_traces(&req),
            ("GET", "/admin/data/stats") => self.admin_data_stats(&req),
            ("GET", "/admin/data/audit") => self.admin_data_audit(&req),
            ("POST", "/admin/data/replicate") => self.admin_data_replicate(&req),
            ("POST", "/admin/data/rebalance") => self.admin_data_rebalance(&req),
            ("GET", "/admin/tenants") => self.admin_tenants_list(&req),
            ("POST", "/admin/tenants") => self.admin_tenants_create(&req),
            ("PUT", p) if p.starts_with("/admin/tenants/") => self.admin_tenants_update(&req, p),
            ("DELETE", p) if p.starts_with("/admin/tenants/") && p.contains("/keys/") => {
                self.admin_tenants_revoke_key(&req, p)
            }
            ("DELETE", p) if p.starts_with("/admin/tenants/") => self.admin_tenants_delete(&req, p),
            ("POST", p) if p.starts_with("/admin/tenants/") => self.admin_tenants_add_key(&req, p),
            _ => Ok(HttpResponse::json(
                404,
                "Not Found",
                r#"{"error":"not_found"}"#,
            )),
        };

        match result {
            Ok(resp) => cors(resp),
            Err(err) => cors(error_response(err)),
        }
    }

    fn authenticate(&self, req: &HttpRequest) -> Result<AuthContext, OntolithError> {
        self.app.authenticator.authenticate(
            req.header("x-ontolith-tenant"),
            req.header("x-ontolith-user"),
            req.header("x-api-key"),
        )
    }

    fn authorize_read(
        &self,
        req: &HttpRequest,
        resource: &str,
        action: &str,
    ) -> Result<AuthContext, OntolithError> {
        let ctx = self.authenticate(req)?;
        authorize(&self.app.audit, &ctx, resource, action, now_ms())?;
        self.enforce_acl(req, &ctx, false)?;
        Ok(ctx)
    }

    fn authorize_admin_view(&self, req: &HttpRequest) -> Result<AuthContext, OntolithError> {
        let ctx = self.authenticate(req)?;
        authorize(&self.app.audit, &ctx, "cluster", "admin", now_ms())?;
        self.enforce_acl(req, &ctx, false)?;
        Ok(ctx)
    }

    fn authorize_admin_mutation(&self, req: &HttpRequest) -> Result<AuthContext, OntolithError> {
        let ctx = self.authenticate(req)?;
        authorize(&self.app.audit, &ctx, "cluster", "admin", now_ms())?;
        self.enforce_acl(req, &ctx, true)?;
        Ok(ctx)
    }

    fn enforce_acl(
        &self,
        req: &HttpRequest,
        ctx: &AuthContext,
        needs_write_key: bool,
    ) -> Result<(), OntolithError> {
        if !self.acl.enabled() {
            return Ok(());
        }

        let provided = req.header(MGMT_KEY_HEADER);
        let allowed = if needs_write_key {
            self.acl.allows_write(provided)
        } else {
            self.acl.allows_read(provided)
        };

        if allowed {
            return Ok(());
        }

        let detail = if needs_write_key {
            "forbidden: management write key required"
        } else {
            "forbidden: management read key required"
        };
        self.app.audit.record(
            now_ms(),
            ctx,
            if needs_write_key { "write" } else { "read" },
            "management",
            AuditOutcome::Deny,
            detail,
        );
        Err(OntolithError::Failed(detail.to_owned()))
    }

    fn admin_health(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "health", "read")?;
        let uptime_ms = now_ms().saturating_sub(self.started_at_ms);
        let runtime_probe =
            probe_runtime_bind(&self.app.bind_address, self.runtime_probe_timeout_ms);
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"status":"ok","service":"ontolith-management-server","uptime_ms":{},"management_bind":{},"runtime_bind":{},"runtime_probe":{{"reachable":{},"latency_ms":{},"error":{}}},"jwt":{},"oidc":{}}}"#,
                uptime_ms,
                json_string(&self.management_bind),
                json_string(&self.app.bind_address),
                runtime_probe.reachable,
                runtime_probe
                    .latency_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "null".to_owned()),
                runtime_probe
                    .error
                    .as_ref()
                    .map(|e| json_string(e))
                    .unwrap_or_else(|| "null".to_owned()),
                json_string(if self.app.authenticator.jwt_enabled() {
                    "on"
                } else {
                    "off"
                }),
                json_string(if self.app.authenticator.jwt_oidc.is_some() {
                    "on"
                } else {
                    "off"
                }),
            ),
        ))
    }

    fn admin_config(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_view(req)?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"management_bind":{},"runtime_bind":{},"storage_backend":{},"data_dir":{},"auth_mode":{},"tenant_mode":{},"audit_path":{},"tls":{},"semantic":{},"tracing":"on","started_at_ms":{}}}"#,
                json_string(&self.management_bind),
                json_string(&self.app.bind_address),
                json_string(self.app.backend.as_str()),
                self.app
                    .data_dir
                    .as_ref()
                    .map(|p| json_string(&p.display().to_string()))
                    .unwrap_or_else(|| "null".to_owned()),
                json_string(match self.app.authenticator.mode {
                    AuthMode::Disabled => "disabled",
                    AuthMode::Enforced => "enforced",
                }),
                json_string(self.app.tenant_mode.as_str()),
                self.app
                    .audit
                    .file_path()
                    .map(|p| json_string(&p))
                    .unwrap_or_else(|| "null".to_owned()),
                if self.tls_enabled {
                    "\"on\""
                } else {
                    "\"off\""
                },
                json_string(if self.app.semantic.is_some() {
                    "on"
                } else {
                    "off"
                }),
                self.started_at_ms,
            ),
        ))
    }

    fn admin_traces(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "metrics", "read")?;
        let limit = req
            .query
            .get("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20)
            .min(200);
        let spans = self.app.traces.spans();
        Ok(HttpResponse::json(
            200,
            "OK",
            render_traces_json(&spans, limit),
        ))
    }

    fn admin_layers(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_view(req)?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"layer_count":9,"layers":[{{"id":"L0","crate":"ontolith-core","domain":"knowledge model"}},{{"id":"L1","crate":"ontolith-rdf","domain":"rdf graph model"}},{{"id":"L2","crate":"ontolith-storage","domain":"storage and transaction kernel"}},{{"id":"L3","crate":"ontolith-query","domain":"sparql parse optimize execute"}},{{"id":"L4","crate":"ontolith-cluster","domain":"cluster consistency and control"}},{{"id":"L5","crate":"ontolith-server","domain":"http gateway and management"}},{{"id":"L6","crate":"ontolith-reasoner","domain":"reasoning extension surface"}},{{"id":"L7","crate":"ontolith-observability","domain":"metrics and runtime signals"}},{{"id":"L8","crate":"ontolith-plugin-api","domain":"plugin contracts"}}],"runtime_bind":{}}}"#,
                json_string(&self.app.bind_address),
            ),
        ))
    }

    /// L8: plugin contract surface — built-in plugin manifests, agent tool
    /// definitions, and the capability set exposed by `ontolith-plugin-api`
    /// (P8-03). Dynamic plugin loading is not wired yet; this endpoint
    /// reports the registered built-in surface so consoles can render the
    /// plugin list, tool contracts, and contract status.
    fn admin_plugins(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "metrics", "read")?;
        Ok(HttpResponse::json(200, "OK", plugins_surface_json()))
    }

    fn admin_monitoring(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "metrics", "read")?;

        // Live request counters live on the gateway process; in split
        // deployments the management process serves no traffic. Pull them
        // from the runtime /metrics (same API key) and fall back to local
        // state when the gateway is unreachable or no key is configured.
        let live = self.fetch_runtime_counters();
        let requests_total = live
            .as_ref()
            .map(|c| c.requests_total)
            .unwrap_or_else(|| self.app.requests_total.load(Ordering::Relaxed));
        let sparql_total = live
            .as_ref()
            .map(|c| c.sparql_total)
            .unwrap_or_else(|| self.app.sparql_total.load(Ordering::Relaxed));
        let sparql_errors = live
            .as_ref()
            .map(|c| c.sparql_errors)
            .unwrap_or_else(|| self.app.sparql_errors.load(Ordering::Relaxed));
        let ingest_total = live
            .as_ref()
            .map(|c| c.ingest_total)
            .unwrap_or_else(|| self.app.ingest_total.load(Ordering::Relaxed));
        let latency_avg_ms = live
            .as_ref()
            .map(RuntimeCounters::latency_avg_ms)
            .unwrap_or_else(|| {
                let latency_count = self.app.latency_count.load(Ordering::Relaxed);
                let latency_sum_ms = self.app.latency_sum_ms.load(Ordering::Relaxed);
                if latency_count > 0 {
                    latency_sum_ms as f64 / latency_count as f64
                } else {
                    0.0
                }
            });

        let mut status_pairs = Vec::new();
        let statuses: Vec<(String, u64)> = match &live {
            Some(c) => c
                .status_counts
                .iter()
                .map(|(code, count)| (code.to_string(), *count))
                .collect(),
            None => self
                .app
                .status_counts
                .lock()
                .map(|map| {
                    map.iter()
                        .map(|(code, count)| (code.to_string(), *count))
                        .collect()
                })
                .unwrap_or_default(),
        };
        for (code, count) in statuses {
            status_pairs.push(format!(r#"{}:{}"#, json_string(&code), count));
        }
        status_pairs.sort();
        let status_map = format!("{{{}}}", status_pairs.join(","));

        let cluster = self.app.cluster.status();
        let leader = cluster
            .leader_id
            .as_ref()
            .map(|id| json_string(id.as_str()))
            .unwrap_or_else(|| "null".to_owned());
        let runtime_probe =
            probe_runtime_bind(&self.app.bind_address, self.runtime_probe_timeout_ms);

        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"requests_total":{},"sparql_total":{},"sparql_errors":{},"ingest_total":{},"latency_avg_ms":{},"http_status_counts":{},"runtime_probe":{{"target":{},"reachable":{},"latency_ms":{},"error":{}}},"cluster":{{"epoch":{},"leader":{},"nodes":{},"healthy":{},"shards":{},"shard_map_epoch":{},"commit_index":{}}}}}"#,
                requests_total,
                sparql_total,
                sparql_errors,
                ingest_total,
                latency_avg_ms,
                status_map,
                json_string(&self.app.bind_address),
                runtime_probe.reachable,
                runtime_probe
                    .latency_ms
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "null".to_owned()),
                runtime_probe
                    .error
                    .as_ref()
                    .map(|e| json_string(e))
                    .unwrap_or_else(|| "null".to_owned()),
                cluster.epoch.get(),
                leader,
                cluster.node_count,
                cluster.healthy_count,
                cluster.shard_count,
                self.app.cluster.shard_map().epoch.get(),
                cluster.commit_index,
            ),
        ))
    }

    fn fetch_runtime_counters(&self) -> Option<RuntimeCounters> {
        let api_key = self.app.authenticator.api_key.as_deref()?;
        let url = format!("http://{}/metrics", self.app.bind_address);
        let headers = [
            ("x-api-key", api_key),
            ("x-ontolith-tenant", "system"),
            ("x-ontolith-user", "management"),
        ];
        let text = fetch_http_get_with_headers(&url, &headers).ok()?;
        Some(parse_prometheus_counters(&text))
    }

    fn admin_data_stats(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "health", "read")?;

        // Data lives on the gateway process (rocksdb); the management process
        // keeps only memory-backed bookkeeping state. Report the runtime's
        // storage stats, falling back to local state when unreachable.
        let (triples, quads, pending_txns, audit_events, backend) = match self.fetch_runtime_stats()
        {
            Some(s) => (
                s.triples,
                s.quads,
                s.pending_txns,
                s.audit_events,
                s.backend,
            ),
            None => {
                let stats = self.app.storage.stats();
                (
                    stats.triple_count,
                    stats.quad_count,
                    stats.pending_transactions,
                    self.app.audit.len() as u64,
                    self.app.backend.as_str().to_owned(),
                )
            }
        };
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"triples":{},"quads":{},"pending_txns":{},"audit_events":{},"storage_backend":{}}}"#,
                triples,
                quads,
                pending_txns,
                audit_events,
                json_string(&backend),
            ),
        ))
    }

    fn fetch_runtime_stats(&self) -> Option<RuntimeStats> {
        let api_key = self.app.authenticator.api_key.as_deref()?;
        let headers = [
            ("x-api-key", api_key),
            ("x-ontolith-tenant", "system"),
            ("x-ontolith-user", "management"),
        ];
        let bind = &self.app.bind_address;
        let health =
            fetch_http_get_with_headers(&format!("http://{bind}/health"), &headers).ok()?;
        let triples = parse_json_u64(&health, "triples")?;
        let quads = parse_json_u64(&health, "quads")?;
        let pending_txns = parse_json_u64(&health, "pending_txns")?;
        let backend = parse_backend_from_health(&health)?;
        let audit_events = fetch_http_get_with_headers(&format!("http://{bind}/metrics"), &headers)
            .map(|text| parse_prometheus_counters(&text).audit_events)
            .unwrap_or_else(|_| self.app.audit.len() as u64);
        Some(RuntimeStats {
            triples,
            quads,
            pending_txns,
            audit_events,
            backend,
        })
    }

    fn admin_data_audit(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "metrics", "read")?;
        let limit = req
            .query
            .get("limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20)
            .min(200);
        let mut events = self.app.audit.events();
        if events.len() > limit {
            events = events.split_off(events.len() - limit);
        }

        let mut body = String::from("[");
        for (idx, event) in events.iter().enumerate() {
            if idx > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#"{{"ts":{},"tenant":{},"user":{},"action":{},"resource":{},"outcome":{},"detail":{}}}"#,
                event.timestamp_ms,
                json_string(&event.tenant),
                json_string(&event.user),
                json_string(&event.action),
                json_string(&event.resource),
                json_string(event.outcome.as_str()),
                json_string(&event.detail),
            ));
        }
        body.push(']');

        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"total":{},"limit":{},"events":{}}}"#,
                self.app.audit.len(),
                limit,
                body,
            ),
        ))
    }

    fn admin_data_replicate(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        // Optional demo append (`?append=1`), mirroring `/cluster/replicate`:
        // drives one raft entry so the multi-node smoke can assert commit
        // propagation through the management API.
        if req
            .query
            .get("append")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false)
        {
            self.app
                .cluster
                .append(LogPayload::Metadata("mgmt-api-append".into()))?;
        }
        let applied = self.app.cluster.replicate_to_followers()?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"applied_entries":{},"leader_index":{},"commit_index":{}}}"#,
                applied,
                self.app.cluster.leader_index(),
                self.app.cluster.commit_index(),
            ),
        ))
    }

    fn admin_data_rebalance(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        let plans = self.app.cluster.rebalance()?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"plans":{},"epoch":{},"shards":{},"shard_map_epoch":{}}}"#,
                plans.len(),
                self.app.cluster.current_epoch().get(),
                self.app.cluster.shard_map().assignments.len(),
                self.app.cluster.shard_map().epoch.get(),
            ),
        ))
    }

    /// Management-plane tenant registry (tenant management). The durable
    /// registry lives in the gateway process (single RocksDB owner, shared
    /// with the gateway authenticator); the management plane enforces the
    /// admin ACL here and proxies the request to the gateway, which enforces
    /// the system-tenant + cluster-admin check.
    fn admin_tenants_list(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_view(req)?;
        self.proxy_tenant_gateway(req, "GET", "/admin/tenants")
    }

    fn admin_tenants_create(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        self.proxy_tenant_gateway(req, "POST", "/admin/tenants")
    }

    fn admin_tenants_update(
        &self,
        req: &HttpRequest,
        path: &str,
    ) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        self.proxy_tenant_gateway(req, "PUT", path)
    }

    fn admin_tenants_delete(
        &self,
        req: &HttpRequest,
        path: &str,
    ) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        self.proxy_tenant_gateway(req, "DELETE", path)
    }

    fn admin_tenants_add_key(
        &self,
        req: &HttpRequest,
        path: &str,
    ) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        self.proxy_tenant_gateway(req, "POST", path)
    }

    fn admin_tenants_revoke_key(
        &self,
        req: &HttpRequest,
        path: &str,
    ) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_mutation(req)?;
        self.proxy_tenant_gateway(req, "DELETE", path)
    }

    /// Forward a tenant-management request to the gateway process with the
    /// gateway's own credentials (legacy API key + `system` tenant).
    fn proxy_tenant_gateway(
        &self,
        req: &HttpRequest,
        method: &str,
        path: &str,
    ) -> Result<HttpResponse, OntolithError> {
        let api_key = self
            .app
            .authenticator
            .api_key
            .as_deref()
            .ok_or_else(|| OntolithError::Failed("tenant gateway auth unavailable".into()))?;
        let headers = [
            ("x-api-key", api_key),
            ("x-ontolith-tenant", "system"),
            ("x-ontolith-user", "management"),
            ("content-type", "application/json"),
        ];
        let body = if matches!(method, "POST" | "PUT") {
            req.body.as_slice()
        } else {
            &[][..]
        };
        let (status, text) = http_exchange(
            method,
            &format!("http://{}{}", self.app.bind_address, path),
            &headers,
            body,
        )
        .map_err(|e| OntolithError::Failed(format!("tenant gateway proxy: {e}")))?;
        let reason = if status >= 400 { "Error" } else { "OK" };
        Ok(HttpResponse::json(status, reason, text))
    }
}

pub fn shared_management_handler(state: Arc<ManagementState>) -> Handler {
    Arc::new(move |req| state.handle(req))
}

pub fn dispatch_for_test(state: &Arc<ManagementState>, req: HttpRequest) -> HttpResponse {
    state.handle(req)
}

pub fn run() -> Result<(), String> {
    // ikc-log-center Rust SDK：LOG_CENTER_URL 配置时启用管理面日志上报。
    crate::logcenter::init("ontolith-management-server");

    let management_bind = env::var(MGMT_BIND_ENV).unwrap_or_else(|_| DEFAULT_MGMT_BIND.to_owned());
    let acl = load_management_acl_from_env();
    let runtime_probe_timeout_ms = load_runtime_probe_timeout_ms();
    let tls = load_tls_config_from_env()?;
    enforce_tls_gate(&management_bind, tls.is_some())?;

    let app = build_gateway_app_state_from_env()?;
    let state = ManagementState::new(
        app,
        management_bind.clone(),
        acl.clone(),
        runtime_probe_timeout_ms,
        tls.is_some(),
    );

    println!(
        "ontolith-management-server starting: bind={}, runtime_bind={}, backend={}, acl_read_key={}, acl_write_key={}, probe_timeout_ms={}, tls={}, jwt={}, oidc={}",
        management_bind,
        state.app.bind_address,
        state.app.backend.as_str(),
        acl.read_key.is_some(),
        acl.write_key.is_some(),
        runtime_probe_timeout_ms,
        if tls.is_some() { "on" } else { "off" },
        if state.app.authenticator.jwt_enabled() {
            "on"
        } else {
            "off"
        },
        if state.app.authenticator.jwt_oidc.is_some() {
            "on"
        } else {
            "off"
        },
    );
    crate::logcenter::emit(
        "INFO",
        "ontolith-management-server",
        &format!(
            "starting: bind={}, runtime_bind={}, backend={}, acl_read_key={}, acl_write_key={}, probe_timeout_ms={}, tls={}, jwt={}, oidc={}",
            management_bind,
            state.app.bind_address,
            state.app.backend.as_str(),
            acl.read_key.is_some(),
            acl.write_key.is_some(),
            runtime_probe_timeout_ms,
            if tls.is_some() { "on" } else { "off" },
            if state.app.authenticator.jwt_enabled() {
                "on"
            } else {
                "off"
            },
            if state.app.authenticator.jwt_oidc.is_some() {
                "on"
            } else {
                "off"
            },
        ),
    );

    let server = match tls {
        Some(tls) => HttpServer::with_tls(shared_management_handler(state), tls),
        None => HttpServer::new(shared_management_handler(state)),
    };
    server
        .serve(&management_bind)
        .map_err(|e| format!("management server listen {}: {e}", management_bind))
}

fn load_tls_config_from_env() -> Result<Option<TlsServerConfig>, String> {
    let cert_path = env::var(TLS_CERT_ENV).ok().filter(|v| !v.trim().is_empty());
    let key_path = env::var(TLS_KEY_ENV).ok().filter(|v| !v.trim().is_empty());
    match (cert_path, key_path) {
        (None, None) => Ok(None),
        (Some(cert), Some(key)) => {
            let cert_pem =
                std::fs::read(&cert).map_err(|e| format!("read TLS cert file {cert}: {e}"))?;
            let key_pem =
                std::fs::read(&key).map_err(|e| format!("read TLS key file {key}: {e}"))?;
            TlsServerConfig::from_pem(&cert_pem, &key_pem).map(Some)
        }
        _ => Err(format!(
            "TLS requires both {TLS_CERT_ENV} and {TLS_KEY_ENV} to be set"
        )),
    }
}

/// R2 gate: a non-loopback management bind must be TLS-terminated.
fn enforce_tls_gate(bind: &str, tls_enabled: bool) -> Result<(), String> {
    if tls_enabled || is_loopback_bind(bind) {
        Ok(())
    } else {
        Err(format!(
            "non-loopback management bind '{bind}' requires TLS (R2 gate): set {TLS_CERT_ENV} and {TLS_KEY_ENV}"
        ))
    }
}

fn is_loopback_bind(bind: &str) -> bool {
    match bind.to_socket_addrs() {
        Ok(addrs) => {
            let mut found = false;
            for addr in addrs {
                found = true;
                if !addr.ip().is_loopback() {
                    return false;
                }
            }
            found
        }
        Err(_) => false,
    }
}

fn load_authenticator() -> Result<HeaderAuthenticator, String> {
    let mode = match env::var(AUTH_MODE_ENV)
        .unwrap_or_else(|_| "disabled".to_owned())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "enforced" => AuthMode::Enforced,
        _ => AuthMode::Disabled,
    };

    let mut authenticator = HeaderAuthenticator {
        mode,
        api_key: env::var(API_KEY_ENV).ok(),
        jwt_secret: env::var(JWT_SECRET_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty()),
        jwt_issuer: env::var(JWT_ISSUER_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty()),
        jwt_audience: env::var(JWT_AUDIENCE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty()),
        jwt_leeway_secs: env::var(JWT_LEEWAY_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0),
        jwt_oidc: load_jwks_from_env()?,
        ..HeaderAuthenticator::default()
    };
    // OIDC policy envs take precedence for the JWKS path only.
    if authenticator.jwt_oidc.is_some() {
        if let Some(issuer) = env::var(OIDC_ISSUER_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            authenticator.jwt_issuer = Some(issuer);
        }
        if let Some(audience) = env::var(OIDC_AUDIENCE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            authenticator.jwt_audience = Some(audience);
        }
    }
    Ok(authenticator)
}

/// In-tree JWKS transport for the server: `file://` snapshots (drill /
/// production pin) and plain-HTTP GET, mirroring the L4 raft transport.
/// `https://` is rejected at load time until a TLS client lands — operators
/// front the IdP with a reverse proxy or mount a `file://` set.
struct JwksUrlFetcher;

impl JwksFetcher for JwksUrlFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        if let Some(path) = url.strip_prefix("file://") {
            std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))
        } else if url.starts_with("http://") {
            fetch_http_get(url)
        } else {
            Err(format!("unsupported jwks scheme in {url:?}"))
        }
    }
}

/// Load the OIDC JWKS verifier from `ONTOLITH_OIDC_JWKS_URL` (R2+ chain).
///
/// Fetches once at startup (fail fast on load/parse errors), then serves a
/// TTL-refreshed cache (`ONTOLITH_OIDC_CACHE_TTL_SECS`, default 300s) so
/// rotated keys are picked up without a restart; a failed refresh keeps
/// serving the last good key set.
fn load_jwks_from_env() -> Result<Option<JwksVerifier>, String> {
    let Some(url) = env::var(OIDC_JWKS_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(None);
    };
    let ttl = match env::var(OIDC_CACHE_TTL_SECS_ENV) {
        Ok(raw) => raw.trim().parse::<u64>().map_err(|_| {
            format!("{OIDC_CACHE_TTL_SECS_ENV} must be an integer number of seconds, got {raw:?}")
        })?,
        Err(_) => 300,
    };
    if url.starts_with("https://") {
        return Err(format!(
            "{OIDC_JWKS_URL_ENV} https:// is not supported by the in-tree client; use file:// or http:// (terminate TLS at a reverse proxy)"
        ));
    }
    if !url.starts_with("file://") && !url.starts_with("http://") {
        return Err(format!(
            "{OIDC_JWKS_URL_ENV} must be file:// or http(s)://, got {url:?}"
        ));
    }
    let fetcher = JwksUrlFetcher;
    let text = fetcher
        .get(&url)
        .map_err(|e| format!("{OIDC_JWKS_URL_ENV} fetch {url}: {e}"))?;
    let jwks = Jwks::from_json(&text)
        .map_err(|e| format!("{OIDC_JWKS_URL_ENV} parse: {}", e.message()))?;
    Ok(Some(JwksVerifier::new(
        CachingJwks::new(jwks, Duration::from_secs(ttl)),
        url,
        Arc::new(fetcher),
    )))
}

/// Minimal synchronous HTTP GET for JWKS (mirrors the L4 raft in-tree
/// transport): resolves host:port, sends `GET`, and parses status +
/// Content-Length into the response body.
fn fetch_http_get(url: &str) -> Result<String, String> {
    fetch_http_get_with_headers(url, &[])
}

/// [`fetch_http_get`] with extra request headers (e.g. API-key auth for the
/// runtime `/metrics` pull).
fn fetch_http_get_with_headers(url: &str, headers: &[(&str, &str)]) -> Result<String, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported scheme in {url:?}"))?;
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let addr = host_port
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| format!("cannot resolve jwks host {host_port:?}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("connect jwks host {host_port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: text/plain\r\nConnection: close\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write jwks request: {e}"))?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err("jwks response headers too large".into());
        }
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("read jwks response: {e}"))?;
        if n == 0 {
            return Err("unexpected EOF before jwks response headers".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed jwks status line: {status_line}"))?;
    if status != 200 {
        return Err(format!("jwks fetch {url} returned HTTP {status}"));
    }
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("read jwks body: {e}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    String::from_utf8(body).map_err(|e| format!("jwks body not utf8: {e}"))
}

/// Minimal HTTP/1.1 exchange for the management→gateway tenant proxy
/// (arbitrary method + body; returns upstream status and body).
fn http_exchange(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<(u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported scheme in {url:?}"))?;
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let addr = host_port
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| format!("cannot resolve host {host_port:?}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("connect {host_port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write request: {e}"))?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err("response headers too large".into());
        }
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("read response: {e}"))?;
        if n == 0 {
            return Err("unexpected EOF before response headers".into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed status line: {status_line}"))?;
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body_buf = buf[header_end + 4..].to_vec();
    while body_buf.len() < content_length {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            break;
        }
        body_buf.extend_from_slice(&tmp[..n]);
    }
    body_buf.truncate(content_length);
    String::from_utf8(body_buf)
        .map(|b| (status, b))
        .map_err(|e| format!("body not utf8: {e}"))
}

/// Live gateway counters snapshot, parsed from the runtime `/metrics`
/// (Prometheus text format) so split deployments report real traffic.
#[derive(Debug, Default)]
struct RuntimeCounters {
    requests_total: u64,
    sparql_total: u64,
    sparql_errors: u64,
    ingest_total: u64,
    latency_sum_ms: u64,
    latency_count: u64,
    status_counts: Vec<(u16, u64)>,
    storage_triples: u64,
    storage_quads: u64,
    storage_pending_txns: u64,
    audit_events: u64,
}

impl RuntimeCounters {
    fn latency_avg_ms(&self) -> f64 {
        if self.latency_count > 0 {
            self.latency_sum_ms as f64 / self.latency_count as f64
        } else {
            0.0
        }
    }
}

fn parse_prometheus_counters(text: &str) -> RuntimeCounters {
    let mut out = RuntimeCounters::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        match name {
            "ontolith_http_requests_total" => out.requests_total = value,
            "ontolith_sparql_requests_total" => out.sparql_total = value,
            "ontolith_sparql_errors_total" => out.sparql_errors = value,
            "ontolith_ingest_requests_total" => out.ingest_total = value,
            "ontolith_http_request_latency_ms_sum" => out.latency_sum_ms = value,
            "ontolith_http_request_latency_ms_count" => out.latency_count = value,
            "ontolith_storage_triples" => out.storage_triples = value,
            "ontolith_storage_quads" => out.storage_quads = value,
            "ontolith_storage_pending_txns" => out.storage_pending_txns = value,
            "ontolith_audit_events" => out.audit_events = value,
            _ => {}
        }
        if let Some(labels) = name.strip_prefix("ontolith_http_responses_total")
            && let Some(status) = parse_status_label(labels)
        {
            out.status_counts.push((status, value));
        }
    }
    out
}

fn parse_status_label(labels: &str) -> Option<u16> {
    let labels = labels.strip_prefix('{')?.strip_suffix('}')?;
    for part in labels.split(',') {
        let (key, value) = part.split_once('=')?;
        if key.trim() == "status" {
            return value.trim().trim_matches('"').parse().ok();
        }
    }
    None
}

/// Runtime storage snapshot for split deployments, parsed from the gateway
/// `/health` (counts + backend) and `/metrics` (audit events).
#[derive(Debug)]
struct RuntimeStats {
    triples: u64,
    quads: u64,
    pending_txns: u64,
    audit_events: u64,
    backend: String,
}

fn parse_json_u64(text: &str, key: &str) -> Option<u64> {
    let pat = format!(r#""{key}":"#);
    let start = text.find(&pat)? + pat.len();
    let rest = &text[start..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    rest[..digits].parse().ok()
}

fn parse_backend_from_health(text: &str) -> Option<String> {
    let key = r#""backend":"#;
    let start = text.find(key)? + key.len();
    let rest = text[start..].strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// `ONTOLITH_TENANT_MODE` → [`TenantMode`] (P5-03). Defaults to `disabled`
/// so legacy single-tenant deployments keep their behavior.
fn load_tenant_mode() -> TenantMode {
    TenantMode::parse(&env::var(TENANT_MODE_ENV).unwrap_or_else(|_| "disabled".to_owned()))
}

fn load_management_acl_from_env() -> ManagementAcl {
    let read_key = env::var(MGMT_READ_KEY_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty());
    let write_key = env::var(MGMT_WRITE_KEY_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty());
    ManagementAcl {
        read_key,
        write_key,
    }
}

fn load_runtime_probe_timeout_ms() -> u64 {
    env::var(MGMT_RUNTIME_PROBE_TIMEOUT_MS_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(300)
}

#[derive(Debug, Clone)]
struct RuntimeProbeResult {
    reachable: bool,
    latency_ms: Option<u64>,
    error: Option<String>,
}

fn probe_runtime_bind(bind: &str, timeout_ms: u64) -> RuntimeProbeResult {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let addrs = match bind.to_socket_addrs() {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(err) => {
            return RuntimeProbeResult {
                reachable: false,
                latency_ms: None,
                error: Some(format!("resolve failed: {err}")),
            };
        }
    };

    if addrs.is_empty() {
        return RuntimeProbeResult {
            reachable: false,
            latency_ms: None,
            error: Some("resolve failed: no socket addresses".to_owned()),
        };
    }

    let mut last_error = None;
    for addr in addrs {
        let started = Instant::now();
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => {
                return RuntimeProbeResult {
                    reachable: true,
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    error: None,
                };
            }
            Err(err) => {
                last_error = Some(format!("{addr}: {err}"));
            }
        }
    }

    RuntimeProbeResult {
        reachable: false,
        latency_ms: None,
        error: last_error,
    }
}

fn load_audit_log_from_env() -> Result<InMemoryAuditLog, OntolithError> {
    let mut audit = InMemoryAuditLog::new();
    if let Some(path) = env::var(AUDIT_PATH_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
    {
        let file = FileAuditLog::open(path)?;
        audit.set_file_sink(file);
    }
    Ok(audit)
}

fn build_managed_app_state(
    bind_address: String,
    auth: HeaderAuthenticator,
    audit: InMemoryAuditLog,
    tenant_mode: TenantMode,
) -> Result<Arc<AppState>, String> {
    let cluster = build_cluster_runtime()?;
    let wants_rocks = env::var(STORAGE_ENV)
        .ok()
        .map(|v| {
            let normalized = v.trim().to_ascii_lowercase();
            normalized == "rocksdb" || normalized == "durable"
        })
        .unwrap_or(false);

    let data_dir = env::var(DATA_DIR_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from);

    #[cfg(feature = "rocksdb-backend")]
    {
        if wants_rocks || data_dir.is_some() {
            let path = data_dir.unwrap_or_else(|| PathBuf::from("./data/ontolith"));
            return AppState::new_rocksdb_with_cluster(
                bind_address,
                auth,
                path,
                audit,
                tenant_mode,
                cluster,
                InferenceConfig::from_env(),
                crate::app::SemanticConfig::from_env(),
            )
            .map_err(|e| e.message().to_owned());
        }
    }

    #[cfg(not(feature = "rocksdb-backend"))]
    {
        if wants_rocks || data_dir.is_some() {
            return Err("rocksdb backend requested but feature is disabled".to_owned());
        }
    }

    Ok(AppState::new_memory_with_cluster(
        bind_address,
        auth,
        audit,
        tenant_mode,
        cluster,
        InferenceConfig::from_env(),
        crate::app::SemanticConfig::from_env(),
    ))
}

/// Build the L5 gateway [`AppState`] from the shared environment contract
/// (`ONTOLITH_BIND`, storage/auth/tenant/audit knobs). Used by the
/// `ontolith-server` binary bootstrap and the management server.
pub(crate) fn build_gateway_app_state_from_env() -> Result<Arc<AppState>, String> {
    let api_bind = env::var(API_BIND_ENV).unwrap_or_else(|_| DEFAULT_API_BIND.to_owned());
    let authenticator = load_authenticator()?;
    let audit = load_audit_log_from_env().map_err(|e| e.message().to_owned())?;
    let tenant_mode = load_tenant_mode();
    build_managed_app_state(api_bind, authenticator, audit, tenant_mode)
}

/// Select the L4 cluster runtime for the management binary.
///
/// `ONTOLITH_CLUSTER_MODE` defaults to `raft` (ADR-0004 M3: the raft-backed
/// runtime is the production default; the in-memory simulator remains the
/// deterministic test/CI harness). `raft` requires the `raft-backend`
/// feature of `ontolith-cluster` (default on).
fn build_cluster_runtime() -> Result<Arc<dyn ontolith_cluster::application::ClusterRuntime>, String>
{
    let mode = env::var(CLUSTER_MODE_ENV)
        .unwrap_or_else(|_| "raft".to_owned())
        .trim()
        .to_ascii_lowercase();
    match mode.as_str() {
        "raft" => {
            #[cfg(feature = "raft-backend")]
            {
                crate::app::default_raft_cluster()
            }
            #[cfg(not(feature = "raft-backend"))]
            {
                Err(
                    "cluster mode 'raft' requested but ontolith-server was built without the raft-backend feature"
                        .to_owned(),
                )
            }
        }
        "memory" | "simulator" | "in-memory" => Ok(crate::app::default_cluster()),
        other => Err(format!(
            "unknown {CLUSTER_MODE_ENV} '{other}' (expected raft|memory)"
        )),
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
    } else if matches!(err, OntolithError::NotFound(_)) {
        (404, "Not Found")
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
            json_string(err.code()),
        ),
    )
}

fn cors(mut resp: HttpResponse) -> HttpResponse {
    resp.headers
        .push(("Access-Control-Allow-Origin".to_owned(), "*".to_owned()));
    resp.headers.push((
        "Access-Control-Allow-Headers".to_owned(),
        "Content-Type, Accept, X-API-Key, X-Ontolith-Tenant, X-Ontolith-User".to_owned(),
    ));
    resp.headers.push((
        "Access-Control-Allow-Methods".to_owned(),
        "GET, POST, OPTIONS".to_owned(),
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

fn capability_json(c: &PluginCapability) -> String {
    let name = match c {
        PluginCapability::StorageBackend => "storage_backend",
        PluginCapability::Parser => "parser",
        PluginCapability::Optimizer => "optimizer",
        PluginCapability::Reasoner => "reasoner",
        PluginCapability::SecurityProvider => "security_provider",
        PluginCapability::Retrieval => "retrieval",
    };
    json_string(name)
}

fn capabilities_json(caps: &[PluginCapability]) -> String {
    let items: Vec<String> = caps.iter().map(capability_json).collect();
    format!("[{}]", items.join(","))
}

fn tools_json(tools: &[ToolDefinition]) -> String {
    let mut out = Vec::with_capacity(tools.len());
    for t in tools {
        let params: Vec<String> = t
            .parameters
            .iter()
            .map(|p| {
                format!(
                    r#"{{"name":{},"description":{},"required":{}}}"#,
                    json_string(&p.name),
                    json_string(&p.description),
                    p.required,
                )
            })
            .collect();
        out.push(format!(
            r#"{{"name":{},"description":{},"parameters":[{}],"capabilities":{}}}"#,
            json_string(&t.name),
            json_string(&t.description),
            params.join(","),
            capabilities_json(&t.capabilities),
        ));
    }
    format!("[{}]", out.join(","))
}

fn plugin_manifest_json(m: &PluginManifest, tools: &[ToolDefinition]) -> String {
    format!(
        r#"{{"id":{},"version":{},"api_version":{},"capabilities":{},"tools":{}}}"#,
        json_string(&m.id.0),
        json_string(&m.version),
        json_string(&m.api_version),
        capabilities_json(&m.capabilities),
        tools_json(tools),
    )
}

fn plugins_surface_json() -> String {
    // Built-in platform plugins registered against the P8-03 contract. The
    // reference `SemanticRetrievalTool` (ontolith-ai) backs the retrieval
    // capability; manifests stay in sync with `ontolith-plugin-api` types.
    let semantic_retrieval_manifest = PluginManifest {
        id: PluginId("ontolith.semantic-retrieval".to_owned()),
        version: "0.1.0".to_owned(),
        api_version: "0.1.0".to_owned(),
        capabilities: vec![PluginCapability::Retrieval],
    };
    let semantic_retrieval_tool = ToolDefinition {
        name: "semantic_retrieval".to_owned(),
        description: "Semantic term retrieval over the RDF store: returns top-k terms ".to_owned()
            + "related to a natural-language query; verification still goes through "
            + "SPARQL/SHACL.",
        parameters: vec![
            ToolParam {
                name: "q".to_owned(),
                description: "query text".to_owned(),
                required: true,
            },
            ToolParam {
                name: "k".to_owned(),
                description: "max hits [1,100]".to_owned(),
                required: false,
            },
        ],
        capabilities: vec![PluginCapability::Retrieval],
    };
    let plugins = plugin_manifest_json(&semantic_retrieval_manifest, &[semantic_retrieval_tool]);
    let capabilities = [
        PluginCapability::StorageBackend,
        PluginCapability::Parser,
        PluginCapability::Optimizer,
        PluginCapability::Reasoner,
        PluginCapability::SecurityProvider,
        PluginCapability::Retrieval,
    ];
    format!(
        r#"{{"status":{},"api_version":{},"plugins":[{}],"capabilities":{},"contracts":{{"manifest":{},"agent_tool":{}}}}}"#,
        json_string("ready"),
        json_string("0.1.0"),
        plugins,
        capabilities_json(&capabilities),
        json_string("PluginManifest (id/version/api_version/capabilities)"),
        json_string("AgentTool::definition (ToolDefinition/ToolParam, deterministic call)"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Serializes env-driven tests (they mutate the same process-wide
    /// `ONTOLITH_OIDC_*` variables).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn req(method: &str, path: &str) -> HttpRequest {
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    fn req_with_key(method: &str, path: &str, key: &str) -> HttpRequest {
        let mut headers = HashMap::new();
        headers.insert("X-Ontolith-Management-Key".to_owned(), key.to_owned());
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            query: HashMap::new(),
            headers,
            body: Vec::new(),
        }
    }

    fn test_state(auth: HeaderAuthenticator) -> Arc<ManagementState> {
        test_state_with_acl(auth, ManagementAcl::default())
    }

    fn test_state_with_acl(auth: HeaderAuthenticator, acl: ManagementAcl) -> Arc<ManagementState> {
        let app = AppState::new_memory_with_audit(
            "127.0.0.1:8080".to_owned(),
            auth,
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
        );
        ManagementState::new(app, "127.0.0.1:9091".to_owned(), acl, 10, false)
    }

    struct StaticFetcher(String);

    impl JwksFetcher for StaticFetcher {
        fn get(&self, _url: &str) -> Result<String, String> {
            Ok(self.0.clone())
        }
    }

    fn jwks_verifier(jwks_json: &str) -> JwksVerifier {
        let jwks = Jwks::from_json(jwks_json).expect("jwks");
        JwksVerifier::new(
            CachingJwks::new(jwks, Duration::from_secs(1)),
            "test://jwks",
            Arc::new(StaticFetcher(jwks_json.to_owned())),
        )
    }

    #[test]
    fn config_endpoint_returns_management_shape() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/admin/config"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"management_bind\""));
        assert!(body.contains("\"storage_backend\""));
        assert!(body.contains("\"tls\":\"off\""));
    }

    /// R3 enterprise hardening: `/admin/config` must never echo secret values
    /// (management ACL keys, API keys, JWT secrets, OIDC client secret).
    #[test]
    fn admin_config_never_leaks_secrets() {
        let read_key = "acl-read-9f3a2c-secret";
        let write_key = "acl-write-7b1e4d-secret";
        let api_key = "api-key-5c8f11-secret";
        let jwt_secret = "jwt-super-secret-0a1b2c3d";
        let auth = HeaderAuthenticator {
            mode: AuthMode::Enforced,
            api_key: Some(api_key.to_owned()),
            jwt_secret: Some(jwt_secret.to_owned()),
            ..HeaderAuthenticator::default()
        };
        let acl = ManagementAcl {
            read_key: Some(read_key.to_owned()),
            write_key: Some(write_key.to_owned()),
        };
        let state = test_state_with_acl(auth, acl);
        let mut req = req_with_key("GET", "/admin/config", read_key);
        req.headers
            .insert("x-api-key".to_owned(), api_key.to_owned());
        req.headers
            .insert("x-ontolith-tenant".to_owned(), "acme".to_owned());
        req.headers
            .insert("x-ontolith-user".to_owned(), "admin".to_owned());
        let resp = dispatch_for_test(&state, req);
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        for secret in [read_key, write_key, api_key, jwt_secret] {
            assert!(
                !body.contains(secret),
                "/admin/config must not leak secret {secret:?}"
            );
        }
        // Startup banner must also be redacted to presence booleans.
        let banner = format!(
            "acl_read_key={}, acl_write_key={}",
            read_key.is_empty(),
            write_key.is_empty()
        );
        assert!(banner.contains("false"));
        assert!(!banner.contains(read_key));
        assert!(!banner.contains(write_key));
    }

    #[test]
    fn tls_gate_allows_loopback_without_tls() {
        assert!(enforce_tls_gate("127.0.0.1:9091", false).is_ok());
        assert!(enforce_tls_gate("localhost:9091", false).is_ok());
    }

    #[test]
    fn tls_gate_allows_non_loopback_with_tls() {
        assert!(enforce_tls_gate("0.0.0.0:9091", true).is_ok());
    }

    #[test]
    fn tls_gate_rejects_non_loopback_without_tls() {
        let err = enforce_tls_gate("0.0.0.0:9091", false).expect_err("must reject");
        assert!(err.contains("R2 gate"), "got: {err}");
        assert!(err.contains("ONTOLITH_TLS_CERT"), "got: {err}");
    }

    #[test]
    fn monitoring_endpoint_returns_ok() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/admin/monitoring"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"requests_total\""));
        assert!(body.contains("\"cluster\""));
    }

    #[test]
    fn traces_endpoint_lists_recorded_spans() {
        let state = test_state(HeaderAuthenticator::default());
        // Generate a trace through the runtime gateway.
        let _ = state.app.handle(req("GET", "/health"));

        let resp = dispatch_for_test(&state, req("GET", "/admin/traces"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"name\":\"http.request\""), "body={body}");
        assert!(body.contains("\"span_count\":2"), "body={body}");
        assert!(body.contains("\"total\":1"), "body={body}");

        // Admin config surfaces the tracing posture.
        let resp = dispatch_for_test(&state, req("GET", "/admin/config"));
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"tracing\":\"on\""), "body={body}");
    }

    #[test]
    fn plugins_endpoint_reports_contract_surface() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/admin/plugins"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"status\":\"ready\""), "body={body}");
        assert!(body.contains("\"api_version\":\"0.1.0\""), "body={body}");
        assert!(body.contains("ontolith.semantic-retrieval"), "body={body}");
        assert!(
            body.contains("\"name\":\"semantic_retrieval\""),
            "body={body}"
        );
        assert!(
            body.contains("\"capabilities\":[\"retrieval\"]"),
            "body={body}"
        );
        assert!(
            !body.contains("[["),
            "no double-wrapped arrays: body={body}"
        );
        assert!(body.contains("\"storage_backend\""), "body={body}");
        assert!(body.contains("\"agent_tool\""), "body={body}");
    }

    #[test]
    fn unknown_endpoint_returns_not_found() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/admin/unknown"));
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn enforced_mode_rejects_missing_headers() {
        let auth = HeaderAuthenticator {
            mode: AuthMode::Enforced,
            api_key: Some("secret".to_owned()),
            ..HeaderAuthenticator::default()
        };
        let state = test_state(auth);
        let resp = dispatch_for_test(&state, req("GET", "/admin/config"));
        assert_eq!(resp.status, 401);
    }

    #[test]
    fn acl_split_allows_read_key_for_read_only_endpoint() {
        let acl = ManagementAcl {
            read_key: Some("read-only".to_owned()),
            write_key: Some("write-admin".to_owned()),
        };
        let state = test_state_with_acl(HeaderAuthenticator::default(), acl);
        let resp = dispatch_for_test(
            &state,
            req_with_key("GET", "/admin/monitoring", "read-only"),
        );
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn acl_split_blocks_write_with_read_key() {
        let acl = ManagementAcl {
            read_key: Some("read-only".to_owned()),
            write_key: Some("write-admin".to_owned()),
        };
        let state = test_state_with_acl(HeaderAuthenticator::default(), acl);
        let resp = dispatch_for_test(
            &state,
            req_with_key("POST", "/admin/data/rebalance", "read-only"),
        );
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn acl_split_allows_write_with_write_key() {
        let acl = ManagementAcl {
            read_key: Some("read-only".to_owned()),
            write_key: Some("write-admin".to_owned()),
        };
        let state = test_state_with_acl(HeaderAuthenticator::default(), acl);
        let resp = dispatch_for_test(
            &state,
            req_with_key("POST", "/admin/data/rebalance", "write-admin"),
        );
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn runtime_probe_succeeds_when_listener_is_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let _ = listener.accept();
        });

        let probe = probe_runtime_bind(&addr.to_string(), 300);
        assert!(probe.reachable);
        assert!(probe.error.is_none());
    }

    #[test]
    fn runtime_probe_reports_unreachable_port() {
        let probe = probe_runtime_bind("127.0.0.1:9", 100);
        assert!(!probe.reachable);
        assert!(probe.error.is_some());
    }

    #[test]
    fn jwks_file_loads_and_bearer_authenticates() {
        use ontolith_security::infrastructure::sign_tenant_token;

        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let secret = "0123456789abcdef";
        let jwks_json =
            r#"{"keys":[{"kty":"oct","kid":"k1","alg":"HS256","k":"MDEyMzQ1Njc4OWFiY2RlZg"}]}"#
                .to_owned();
        let file = tempfile::NamedTempFile::new().expect("temp jwks");
        std::fs::write(file.path(), jwks_json).expect("write jwks");

        unsafe {
            std::env::set_var(
                OIDC_JWKS_URL_ENV,
                format!("file://{}", file.path().display()),
            );
            std::env::set_var(OIDC_ISSUER_ENV, "https://idp.example");
            std::env::set_var(OIDC_AUDIENCE_ENV, "ontolith-server");
            std::env::set_var(AUTH_MODE_ENV, "enforced");
            std::env::set_var(JWT_LEEWAY_ENV, "0");
        }
        let auth = load_authenticator().expect("load authenticator");
        unsafe {
            std::env::remove_var(OIDC_JWKS_URL_ENV);
            std::env::remove_var(OIDC_ISSUER_ENV);
            std::env::remove_var(OIDC_AUDIENCE_ENV);
            std::env::remove_var(AUTH_MODE_ENV);
            std::env::remove_var(JWT_LEEWAY_ENV);
        }
        assert!(auth.jwt_oidc.is_some(), "jwks not loaded");
        assert_eq!(auth.jwt_issuer.as_deref(), Some("https://idp.example"));
        assert_eq!(auth.jwt_audience.as_deref(), Some("ontolith-server"));

        let token = sign_tenant_token(
            "acme",
            "u-42",
            secret,
            "https://idp.example",
            "ontolith-server",
            3600,
        )
        .expect("sign token");
        let ctx = auth
            .authenticate_with_bearer(None, None, None, Some(&format!("Bearer {token}")))
            .expect("bearer auth");
        assert_eq!(ctx.tenant, ontolith_security::domain::TenantId::new("acme"));
        assert_eq!(ctx.user, ontolith_security::domain::UserId::new("u-42"));
    }

    #[test]
    fn jwks_http_fetch_roundtrip() {
        let jwks_json =
            r#"{"keys":[{"kty":"oct","kid":"k1","alg":"HS256","k":"MDEyMzQ1Njc4OWFiY2RlZg"}]}"#;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        jwks_json.len(),
                        jwks_json
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
            }
        });

        let body = fetch_http_get(&format!("http://{addr}/jwks")).expect("fetch jwks");
        assert!(body.contains("\"keys\""), "body={body}");
        let jwks = Jwks::from_json(&body).expect("parse jwks");
        assert_eq!(jwks.keys.len(), 1);
    }

    #[test]
    fn jwks_https_rejected_with_clear_message() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var(
                OIDC_JWKS_URL_ENV,
                "https://idp.example/.well-known/jwks.json",
            );
        }
        let err = load_jwks_from_env().expect_err("must reject https");
        unsafe {
            std::env::remove_var(OIDC_JWKS_URL_ENV);
        }
        assert!(err.contains("https://"), "got: {err}");
        assert!(err.contains(OIDC_JWKS_URL_ENV), "got: {err}");
    }

    #[test]
    fn health_reports_jwt_oidc_posture() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/health"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"jwt\":\"off\""), "body={body}");
        assert!(body.contains("\"oidc\":\"off\""), "body={body}");

        let verifier = jwks_verifier(
            r#"{"keys":[{"kty":"oct","kid":"k1","alg":"HS256","k":"MDEyMzQ1Njc4OWFiY2RlZg"}]}"#,
        );
        let auth = HeaderAuthenticator {
            jwt_oidc: Some(verifier),
            ..HeaderAuthenticator::default()
        };
        let state = test_state(auth);
        let resp = dispatch_for_test(&state, req("GET", "/health"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"jwt\":\"on\""), "body={body}");
        assert!(body.contains("\"oidc\":\"on\""), "body={body}");
    }

    #[test]
    fn parses_runtime_counters_from_prometheus_text() {
        let text = "# TYPE ontolith_http_requests_total counter\n\
            ontolith_http_requests_total 42 1000\n\
            ontolith_sparql_requests_total 7 1000\n\
            ontolith_sparql_errors_total 2 1000\n\
            ontolith_ingest_requests_total 3 1000\n\
            ontolith_http_request_latency_ms_sum 560 1000\n\
            ontolith_http_request_latency_ms_count 10 1000\n\
            ontolith_storage_triples 10008 1000\n\
            ontolith_storage_quads 0 1000\n\
            ontolith_storage_pending_txns 0 1000\n\
            ontolith_audit_events 21 1000\n\
            ontolith_http_responses_total{status=\"200\"} 9 1000\n\
            ontolith_http_responses_total{status=\"500\"} 1 1000\n";
        let counters = parse_prometheus_counters(text);
        assert_eq!(counters.requests_total, 42);
        assert_eq!(counters.sparql_total, 7);
        assert_eq!(counters.sparql_errors, 2);
        assert_eq!(counters.ingest_total, 3);
        assert_eq!(counters.latency_sum_ms, 560);
        assert_eq!(counters.latency_count, 10);
        assert_eq!(counters.latency_avg_ms(), 56.0);
        assert_eq!(counters.storage_triples, 10008);
        assert_eq!(counters.storage_quads, 0);
        assert_eq!(counters.storage_pending_txns, 0);
        assert_eq!(counters.audit_events, 21);
        assert_eq!(counters.status_counts, vec![(200, 9), (500, 1)]);
    }

    #[test]
    fn parses_runtime_stats_from_health_json() {
        let health = r#"{"status":"ok","layer":"L5","bind":"127.0.0.1:8080","backend":"rocksdb","triples":10009,"quads":0,"pending_txns":0,"auth_mode":"enforced","tenant_mode":"disabled","jwt":"off","oidc":"off","semantic":"off","tracing":"on","data_dir":"/home/ontolith/prod/data"}"#;
        assert_eq!(parse_json_u64(health, "triples"), Some(10009));
        assert_eq!(parse_json_u64(health, "quads"), Some(0));
        assert_eq!(parse_json_u64(health, "pending_txns"), Some(0));
        assert_eq!(parse_json_u64(health, "missing"), None);
        assert_eq!(
            parse_backend_from_health(health).as_deref(),
            Some("rocksdb")
        );
    }

    fn req_body(method: &str, path: &str, body: &str, key: Option<&str>) -> HttpRequest {
        let mut headers = HashMap::new();
        if let Some(k) = key {
            headers.insert("X-Ontolith-Management-Key".to_owned(), k.to_owned());
        }
        HttpRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            query: HashMap::new(),
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    /// Management tenant routes proxy to the gateway process (the single
    /// owner of the durable registry). Spins up a real gateway HTTP server
    /// so the proxy path + shared-store auth are exercised end to end.
    #[test]
    fn tenant_admin_proxies_to_gateway_and_acl_holds() {
        let gateway_key = "gw-secret";
        let gateway = AppState::new_memory_with_audit(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator {
                mode: AuthMode::Enforced,
                api_key: Some(gateway_key.to_owned()),
                ..HeaderAuthenticator::default()
            },
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let bind = format!("127.0.0.1:{}", addr.port());
        let server = HttpServer::new(crate::app::shared_handler(Arc::clone(&gateway)));
        let stop = server.stop_flag();
        let bind_for_server = bind.clone();
        let handle =
            std::thread::spawn(move || server.serve(&bind_for_server).expect("serve gateway"));
        // Wait for the gateway listener to accept before proxying.
        for _ in 0..100 {
            if std::net::TcpStream::connect(&bind).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let gate = Arc::clone(&gateway);

        // Management state points its runtime probe at the live gateway.
        // Management plane authenticates its own clients; the gateway key is
        // kept for the proxy hop. Disabled mode here isolates the ACL check.
        let mg_app = AppState::new_memory_with_audit(
            bind.clone(),
            HeaderAuthenticator {
                mode: AuthMode::Disabled,
                api_key: Some(gateway_key.to_owned()),
                ..HeaderAuthenticator::default()
            },
            InMemoryAuditLog::new(),
            TenantMode::Disabled,
        );
        let acl = ManagementAcl {
            read_key: Some("r".to_owned()),
            write_key: Some("w".to_owned()),
        };
        let state = ManagementState::new(mg_app, "127.0.0.1:9091".to_owned(), acl, 10, false);

        // Read key cannot mutate.
        let resp = dispatch_for_test(
            &state,
            req_body(
                "POST",
                "/admin/tenants",
                r#"{"id":"acme","name":"Acme","generate_key":true}"#,
                Some("r"),
            ),
        );
        assert_eq!(resp.status, 403, "read key must not mutate");

        // Write key proxies to the gateway; the created key authenticates
        // against the gateway's own authenticator (same store).
        let resp = dispatch_for_test(
            &state,
            req_body(
                "POST",
                "/admin/tenants",
                r#"{"id":"acme","name":"Acme","generate_key":true}"#,
                Some("w"),
            ),
        );
        assert_eq!(
            resp.status,
            201,
            "body={}",
            String::from_utf8_lossy(&resp.body)
        );
        let body = String::from_utf8_lossy(&resp.body).to_string();
        let start = body.find("\"api_key\":\"").expect("api_key") + "\"api_key\":\"".len();
        let end = body[start..].find('"').expect("closing quote");
        let raw = &body[start..start + end];
        assert!(raw.starts_with("ontk_"));

        let ctx = gate
            .authenticator
            .authenticate(None, None, Some(raw))
            .expect("gateway resolves tenant key");
        assert_eq!(ctx.tenant.as_str(), "acme");

        // Read path proxies too.
        let resp = dispatch_for_test(&state, req_with_key("GET", "/admin/tenants", "r"));
        assert_eq!(
            resp.status,
            200,
            "body={}",
            String::from_utf8_lossy(&resp.body)
        );
        assert!(String::from_utf8_lossy(&resp.body).contains("\"id\":\"acme\""));

        stop.store(false, Ordering::SeqCst);
        // Wake the blocked `accept` so the serve loop observes the stop flag.
        let _ = std::net::TcpStream::connect(&bind);
        handle.join().expect("gateway server thread");
    }
}
