//! gRPC access boundary (P5-01): SPARQL gateway over tonic (HTTP/2).
//!
//! Mirrors the HTTP gateway: auth comes from metadata headers
//! (`x-ontolith-tenant`/`x-ontolith-user`/`x-api-key`/`authorization`),
//! `traceparent` metadata continues the W3C trace, and every request is
//! audited and recorded as spans in the shared trace store.

use crate::app::{AppState, explain_json, parse_consistency, sparql_results_json};
use crate::http::now_ms;
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;
use ontolith_observability::domain::{SpanEvent, SpanName, SpanStatus, TraceContext};
use ontolith_observability::infrastructure::{
    TraceScope, current_trace, format_traceparent, generate_span_id, generate_trace_id,
    parse_traceparent,
};
use ontolith_security::application::{Authenticator, authorize};
use ontolith_security::domain::{AuthContext, AuthMode};
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod sparql {
    tonic::include_proto!("ontolith.v1");
}

use sparql::sparql_service_server::{SparqlService as SparqlServiceTrait, SparqlServiceServer};
use sparql::{HealthRequest, HealthResponse, QueryRequest, QueryResponse};

#[derive(Clone)]
pub struct SparqlGateway {
    app: Arc<AppState>,
}

impl SparqlGateway {
    pub fn new(app: Arc<AppState>) -> Self {
        Self { app }
    }

    #[allow(clippy::result_large_err)] // tonic::Status is the idiomatic gRPC error type
    fn authenticate(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<AuthContext, Status> {
        let tenant = metadata
            .get("x-ontolith-tenant")
            .and_then(|value| value.to_str().ok());
        let user = metadata
            .get("x-ontolith-user")
            .and_then(|value| value.to_str().ok());
        let api_key = metadata
            .get("x-api-key")
            .and_then(|value| value.to_str().ok());
        let authorization = metadata
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        self.app
            .authenticator
            .authenticate_with_bearer(tenant, user, api_key, authorization)
            .map_err(status_err)
    }
}

fn status_err(err: OntolithError) -> Status {
    let message = err.message();
    if message.starts_with("unauthorized") {
        Status::unauthenticated(message)
    } else if message.starts_with("forbidden") {
        Status::permission_denied(message)
    } else {
        Status::internal(message)
    }
}

fn record_span(
    app: &AppState,
    name: &str,
    start_ms: u64,
    status: SpanStatus,
    attributes: Vec<(String, String)>,
) {
    if let Some(trace) = current_trace() {
        let _ = app.traces.record(SpanEvent {
            trace_id: trace.trace_id,
            span_id: generate_span_id(),
            parent_span_id: Some(trace.span_id),
            name: SpanName(name.into()),
            start_ms,
            duration_ms: now_ms() - start_ms,
            status,
            attributes,
        });
    }
}

#[tonic::async_trait]
impl SparqlServiceTrait for SparqlGateway {
    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        // Tracing (P5-05): continue the upstream trace or start a new one.
        let upstream = parse_traceparent(
            request
                .metadata()
                .get("traceparent")
                .and_then(|value| value.to_str().ok()),
        );
        let trace_id = upstream
            .as_ref()
            .map(|context| context.trace_id.clone())
            .unwrap_or_else(generate_trace_id);
        let root_span_id = generate_span_id();
        let parent_span_id = upstream.as_ref().map(|context| context.span_id.clone());
        let _scope = TraceScope::enter(TraceContext {
            trace_id: trace_id.clone(),
            span_id: root_span_id.clone(),
        });
        let root_start = now_ms();

        let auth_start = now_ms();
        let ctx = self.authenticate(request.metadata());
        record_span(
            &self.app,
            "http.auth",
            auth_start,
            if ctx.is_ok() {
                SpanStatus::Ok
            } else {
                SpanStatus::Error
            },
            vec![],
        );
        let ctx = ctx?;

        let req = request.into_inner();
        let action = if req.explain { "explain" } else { "query" };
        authorize(&self.app.audit, &ctx, "sparql", action, now_ms()).map_err(status_err)?;

        let consistency = if req.consistency.is_empty() {
            ConsistencyLevel::Strong
        } else {
            parse_consistency(&req.consistency)
        };
        let timeout_ms = if req.timeout_ms > 0 {
            Some(req.timeout_ms)
        } else {
            None
        };
        let exec_start = now_ms();
        let executed: Result<String, OntolithError> = if req.explain {
            self.app
                .explain_sparql(&ctx, &req.query, timeout_ms, consistency)
                .map(|plan| explain_json(&plan, ctx.tenant.as_str(), consistency))
        } else {
            self.app
                .execute_sparql_with_inference(
                    &ctx,
                    &req.query,
                    timeout_ms,
                    consistency,
                    &self.app.inference,
                )
                .map(|outcome| {
                    sparql_results_json(
                        &outcome.result,
                        &ctx,
                        consistency,
                        outcome.reasoning.as_ref(),
                    )
                })
        };
        let executed = executed.map_err(status_err);
        record_span(
            &self.app,
            "sparql.execute",
            exec_start,
            if executed.is_ok() {
                SpanStatus::Ok
            } else {
                SpanStatus::Error
            },
            vec![
                ("kind".into(), action.to_owned()),
                ("tenant".into(), ctx.tenant.as_str().to_owned()),
            ],
        );
        let body = executed?;

        record_span(
            &self.app,
            "grpc.query",
            root_start,
            SpanStatus::Ok,
            vec![
                ("tenant".into(), ctx.tenant.as_str().to_owned()),
                ("explain".into(), req.explain.to_string()),
            ],
        );
        // Record the root span (same shape as the HTTP gateway).
        let _ = self.app.traces.record(SpanEvent {
            trace_id: trace_id.clone(),
            span_id: root_span_id.clone(),
            parent_span_id,
            name: SpanName("grpc.request".into()),
            start_ms: root_start,
            duration_ms: now_ms() - root_start,
            status: SpanStatus::Ok,
            attributes: vec![
                ("rpc".into(), "ontolith.v1.SparqlService/Query".into()),
                ("tenant".into(), ctx.tenant.as_str().to_owned()),
            ],
        });
        let mut response = Response::new(QueryResponse {
            ok: true,
            http_status: 200,
            body,
            error: String::new(),
        });
        response.metadata_mut().insert(
            "traceparent",
            format_traceparent(&trace_id, &root_span_id)
                .parse()
                .expect("traceparent is ascii"),
        );
        Ok(response)
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let ctx = self.authenticate(request.metadata())?;
        authorize(&self.app.audit, &ctx, "health", "read", now_ms()).map_err(status_err)?;
        // Readiness: storage stats callable.
        let _ = self.app.storage.stats();
        Ok(Response::new(HealthResponse {
            status: "ok".into(),
            backend: self.app.backend.as_str().into(),
            tenant_mode: self.app.tenant_mode.as_str().into(),
            auth_mode: match self.app.authenticator.mode {
                AuthMode::Disabled => "disabled",
                AuthMode::Enforced => "enforced",
            }
            .into(),
            jwt: if self.app.authenticator.jwt_secret.is_some() {
                "on"
            } else {
                "off"
            }
            .into(),
            tracing: "on".into(),
        }))
    }
}

/// Serve the gRPC access boundary on a dedicated tokio worker thread.
pub fn serve_grpc(
    app: Arc<AppState>,
    bind: SocketAddr,
) -> Result<std::thread::JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name("ontolith-grpc".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("build tokio runtime for gRPC");
            let result = runtime.block_on(async move {
                Server::builder()
                    .add_service(SparqlServiceServer::new(SparqlGateway::new(app)))
                    .serve(bind)
                    .await
            });
            if let Err(err) = result {
                eprintln!("ontolith-grpc serve {bind} failed: {err}");
            }
        })
        .map_err(|err| format!("spawn gRPC server: {err}"))
}

#[cfg(all(test, feature = "grpc-backend"))]
mod tests {
    use super::*;
    use crate::app::AppState;
    use ontolith_security::application::{HeaderAuthenticator, InMemoryAuditLog};
    use ontolith_security::domain::{AuthMode, TenantMode};
    use sparql::sparql_service_client::SparqlServiceClient;
    use std::time::Duration;

    fn test_app() -> Arc<AppState> {
        AppState::new_memory(
            "127.0.0.1:8080".to_owned(),
            HeaderAuthenticator::default(),
        )
    }

    fn enforced_app() -> Arc<AppState> {
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

    /// Memory app with the forward-chaining inference posture enabled.
    fn inference_app() -> Arc<AppState> {
        use crate::app::{StorageBackendKind, default_cluster};
        use crate::reasoning::InferenceConfig;
        use ontolith_reasoner::domain::InferenceMode;
        use ontolith_storage::application::{DictionaryCodec, StorageEngine, TripleRepository};
        use ontolith_storage::infrastructure::{
            EngineTripleRepository, InMemoryDictionary, InMemoryStorageEngine,
        };
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
            InferenceConfig::new(InferenceMode::ForwardChaining, 64, None),
        )
    }

    fn start_server(app: Arc<AppState>) -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let handle = serve_grpc(app, addr).expect("spawn gRPC server");
        for _ in 0..100 {
            if std::net::TcpStream::connect(addr).is_ok() {
                return (addr, handle);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("gRPC server did not become ready");
    }

    fn connect(
        addr: SocketAddr,
        rt: &tokio::runtime::Runtime,
    ) -> SparqlServiceClient<tonic::transport::Channel> {
        let endpoint =
            tonic::transport::Endpoint::from_shared(format!("http://{addr}")).expect("endpoint");
        rt.block_on(async { SparqlServiceClient::connect(endpoint).await.expect("connect") })
    }

    fn query_req(query: &str) -> Request<QueryRequest> {
        Request::new(QueryRequest {
            query: query.to_owned(),
            format: String::new(),
            explain: false,
            timeout_ms: 0,
            consistency: String::new(),
        })
    }

    #[test]
    fn grpc_query_roundtrip_insert_and_select() {
        let (addr, server) = start_server(test_app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut client = connect(addr, &rt);

        let insert = rt
            .block_on(client.query(query_req(
                "INSERT DATA { <http://ex.org/a> <http://ex.org/p> \"c\" }",
            )))
            .expect("insert must succeed");
        assert!(insert.get_ref().ok);
        assert_eq!(insert.get_ref().http_status, 200);

        let select = rt
            .block_on(client.query(query_req(
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s <http://ex.org/p> ?o }",
            )))
            .expect("select must succeed");
        let body = &select.get_ref().body;
        assert!(body.contains("\"c\""), "body={body}");

        // The response carries a traceparent for downstream propagation.
        assert!(select.metadata().get("traceparent").is_some());
        drop(rt);
        drop(server);
    }

    #[test]
    fn grpc_query_runs_inference_when_enabled() {
        let (addr, server) = start_server(inference_app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut client = connect(addr, &rt);

        let insert = rt
            .block_on(client.query(query_req(
                "INSERT DATA { <http://ex.org/A> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex.org/B> . <http://ex.org/x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/A> }",
            )))
            .expect("insert must succeed");
        assert!(insert.get_ref().ok);

        let select = rt
            .block_on(client.query(query_req(
                "SELECT ?s WHERE { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex.org/B> }",
            )))
            .expect("select must succeed");
        let body = &select.get_ref().body;
        assert!(body.contains("\"row_count\":1"), "body={body}");
        assert!(body.contains("\"reasoning\""), "body={body}");
        assert!(body.contains("\"mode\":\"forward\""), "body={body}");
        drop(rt);
        drop(server);
    }

    #[test]
    fn grpc_query_rejects_missing_credentials_in_enforced_mode() {
        let (addr, server) = start_server(enforced_app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut client = connect(addr, &rt);

        let err = rt
            .block_on(client.query(query_req("SELECT * WHERE { ?s ?p ?o }")))
            .expect_err("enforced mode must reject missing credentials");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);

        // With credentials the same call succeeds.
        let mut req = query_req("SELECT * WHERE { ?s ?p ?o } LIMIT 1");
        req.metadata_mut()
            .insert("x-ontolith-tenant", "acme".parse().unwrap());
        req.metadata_mut()
            .insert("x-ontolith-user", "alice".parse().unwrap());
        req.metadata_mut()
            .insert("x-api-key", "s3cret".parse().unwrap());
        let ok = rt.block_on(client.query(req)).expect("authenticated call");
        assert!(ok.get_ref().ok);
        drop(rt);
        drop(server);
    }

    #[test]
    fn grpc_tenant_isolation_rejects_cross_tenant_graph() {
        let (addr, server) = start_server(enforced_app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut client = connect(addr, &rt);

        let mut req = query_req(
            "SELECT * WHERE { GRAPH <urn:tenant:other> { ?s ?p ?o } }",
        );
        req.metadata_mut()
            .insert("x-ontolith-tenant", "acme".parse().unwrap());
        req.metadata_mut()
            .insert("x-ontolith-user", "alice".parse().unwrap());
        req.metadata_mut()
            .insert("x-api-key", "s3cret".parse().unwrap());
        let err = rt
            .block_on(client.query(req))
            .expect_err("cross-tenant graph must be rejected");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        drop(rt);
        drop(server);
    }

    #[test]
    fn grpc_health_reports_status() {
        let (addr, server) = start_server(test_app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut client = connect(addr, &rt);
        let health = rt
            .block_on(client.health(Request::new(HealthRequest {})))
            .expect("health must succeed");
        assert_eq!(health.get_ref().status, "ok");
        assert_eq!(health.get_ref().backend, "memory");
        assert_eq!(health.get_ref().tracing, "on");
        drop(rt);
        drop(server);
    }
}
