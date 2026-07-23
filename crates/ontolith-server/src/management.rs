//! Management server for unified control-plane operations.

use crate::app::AppState;
use crate::http::{Handler, HttpRequest, HttpResponse, HttpServer, now_ms};
use ontolith_cluster::application::{MetadataService, RebalanceService, Replicator};
use ontolith_core::error::OntolithError;
use ontolith_security::application::{
    Authenticator, HeaderAuthenticator, InMemoryAuditLog, authorize,
};
use ontolith_security::domain::{AuditOutcome, AuthContext, AuthMode};
use ontolith_security::infrastructure::FileAuditLog;
use std::env;
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
const API_KEY_ENV: &str = "ONTOLITH_API_KEY";
const AUDIT_PATH_ENV: &str = "ONTOLITH_AUDIT_PATH";
const MGMT_READ_KEY_ENV: &str = "ONTOLITH_MANAGEMENT_READ_KEY";
const MGMT_WRITE_KEY_ENV: &str = "ONTOLITH_MANAGEMENT_WRITE_KEY";
const MGMT_KEY_HEADER: &str = "x-ontolith-management-key";
const MGMT_RUNTIME_PROBE_TIMEOUT_MS_ENV: &str = "ONTOLITH_MANAGEMENT_PROBE_TIMEOUT_MS";

const DEFAULT_MGMT_BIND: &str = "127.0.0.1:9091";
const DEFAULT_API_BIND: &str = "127.0.0.1:8080";

pub struct ManagementState {
    app: Arc<AppState>,
    management_bind: String,
    started_at_ms: u64,
    acl: ManagementAcl,
    runtime_probe_timeout_ms: u64,
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
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            management_bind,
            started_at_ms: now_ms(),
            acl,
            runtime_probe_timeout_ms,
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
            ("GET", "/admin/monitoring") => self.admin_monitoring(&req),
            ("GET", "/admin/data/stats") => self.admin_data_stats(&req),
            ("GET", "/admin/data/audit") => self.admin_data_audit(&req),
            ("POST", "/admin/data/replicate") => self.admin_data_replicate(&req),
            ("POST", "/admin/data/rebalance") => self.admin_data_rebalance(&req),
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
                r#"{{"status":"ok","service":"ontolith-management-server","uptime_ms":{},"management_bind":{},"runtime_bind":{},"runtime_probe":{{"reachable":{},"latency_ms":{},"error":{}}}}}"#,
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
            ),
        ))
    }

    fn admin_config(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_admin_view(req)?;
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"management_bind":{},"runtime_bind":{},"storage_backend":{},"data_dir":{},"auth_mode":{},"audit_path":{},"started_at_ms":{}}}"#,
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
                self.app
                    .audit
                    .file_path()
                    .map(|p| json_string(&p))
                    .unwrap_or_else(|| "null".to_owned()),
                self.started_at_ms,
            ),
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

    fn admin_monitoring(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "metrics", "read")?;

        let requests_total = self.app.requests_total.load(Ordering::Relaxed);
        let sparql_total = self.app.sparql_total.load(Ordering::Relaxed);
        let sparql_errors = self.app.sparql_errors.load(Ordering::Relaxed);
        let ingest_total = self.app.ingest_total.load(Ordering::Relaxed);
        let latency_count = self.app.latency_count.load(Ordering::Relaxed);
        let latency_sum_ms = self.app.latency_sum_ms.load(Ordering::Relaxed);
        let latency_avg_ms = if latency_count > 0 {
            latency_sum_ms as f64 / latency_count as f64
        } else {
            0.0
        };

        let mut status_pairs = Vec::new();
        if let Ok(statuses) = self.app.status_counts.lock() {
            for (code, count) in statuses.iter() {
                status_pairs.push(format!(r#"{}:{}"#, json_string(&code.to_string()), count));
            }
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
                r#"{{"requests_total":{},"sparql_total":{},"sparql_errors":{},"ingest_total":{},"latency_avg_ms":{},"http_status_counts":{},"runtime_probe":{{"target":{},"reachable":{},"latency_ms":{},"error":{}}},"cluster":{{"epoch":{},"leader":{},"nodes":{},"healthy":{},"shards":{},"commit_index":{}}}}}"#,
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
                cluster.commit_index,
            ),
        ))
    }

    fn admin_data_stats(&self, req: &HttpRequest) -> Result<HttpResponse, OntolithError> {
        let _ = self.authorize_read(req, "health", "read")?;
        let stats = self.app.storage.stats();
        Ok(HttpResponse::json(
            200,
            "OK",
            format!(
                r#"{{"triples":{},"quads":{},"pending_txns":{},"audit_events":{},"storage_backend":{}}}"#,
                stats.triple_count,
                stats.quad_count,
                stats.pending_transactions,
                self.app.audit.len(),
                json_string(self.app.backend.as_str()),
            ),
        ))
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
                r#"{{"plans":{},"epoch":{},"shards":{}}}"#,
                plans.len(),
                self.app.cluster.current_epoch().get(),
                self.app.cluster.shard_map().assignments.len(),
            ),
        ))
    }
}

pub fn shared_management_handler(state: Arc<ManagementState>) -> Handler {
    Arc::new(move |req| state.handle(req))
}

pub fn dispatch_for_test(state: &Arc<ManagementState>, req: HttpRequest) -> HttpResponse {
    state.handle(req)
}

pub fn run() -> Result<(), String> {
    let management_bind = env::var(MGMT_BIND_ENV).unwrap_or_else(|_| DEFAULT_MGMT_BIND.to_owned());
    let api_bind = env::var(API_BIND_ENV).unwrap_or_else(|_| DEFAULT_API_BIND.to_owned());
    let acl = load_management_acl_from_env();
    let runtime_probe_timeout_ms = load_runtime_probe_timeout_ms();

    let authenticator = load_authenticator();
    let audit = load_audit_log_from_env().map_err(|e| e.message().to_owned())?;
    let app = build_managed_app_state(api_bind, authenticator, audit)?;
    let state = ManagementState::new(
        app,
        management_bind.clone(),
        acl.clone(),
        runtime_probe_timeout_ms,
    );

    println!(
        "ontolith-management-server starting: bind={}, runtime_bind={}, backend={}, acl_read_key={}, acl_write_key={}, probe_timeout_ms={}",
        management_bind,
        state.app.bind_address,
        state.app.backend.as_str(),
        acl.read_key.is_some(),
        acl.write_key.is_some(),
        runtime_probe_timeout_ms,
    );

    let server = HttpServer::new(shared_management_handler(state));
    server
        .serve(&management_bind)
        .map_err(|e| format!("management server listen {}: {e}", management_bind))
}

fn load_authenticator() -> HeaderAuthenticator {
    let mode = match env::var(AUTH_MODE_ENV)
        .unwrap_or_else(|_| "disabled".to_owned())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "enforced" => AuthMode::Enforced,
        _ => AuthMode::Disabled,
    };

    HeaderAuthenticator {
        mode,
        api_key: env::var(API_KEY_ENV).ok(),
        ..HeaderAuthenticator::default()
    }
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
) -> Result<Arc<AppState>, String> {
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
            return AppState::new_rocksdb_with_audit(bind_address, auth, path, audit)
                .map_err(|e| e.message().to_owned());
        }
    }

    #[cfg(not(feature = "rocksdb-backend"))]
    {
        if wants_rocks || data_dir.is_some() {
            return Err("rocksdb backend requested but feature is disabled".to_owned());
        }
    }

    Ok(AppState::new_memory_with_audit(bind_address, auth, audit))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        );
        ManagementState::new(app, "127.0.0.1:9091".to_owned(), acl, 10)
    }

    #[test]
    fn config_endpoint_returns_management_shape() {
        let state = test_state(HeaderAuthenticator::default());
        let resp = dispatch_for_test(&state, req("GET", "/admin/config"));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).expect("valid utf8");
        assert!(body.contains("\"management_bind\""));
        assert!(body.contains("\"storage_backend\""));
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
}
