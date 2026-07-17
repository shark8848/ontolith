use crate::application::{QueryExecutor, QueryPlanner, QueryReadService};
use crate::domain::{QueryKind, QueryPlan, QueryPlanId, QueryRequest, QueryResultSummary};
use ontolith_core::domain::NodeId;
use ontolith_core::error::OntolithError;
use ontolith_storage::application::TripleRepository;
use std::sync::Arc;
use std::time::Instant;

fn parse_subject_hint(query: &str) -> Result<Option<NodeId>, OntolithError> {
    let normalized = query.to_ascii_lowercase();
    let marker = "subject=";
    let Some(marker_pos) = normalized.find(marker) else {
        return Ok(None);
    };

    let start = marker_pos + marker.len();
    let rest = &normalized[start..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err(OntolithError::InvalidState("invalid subject hint"));
    }

    let value = digits
        .parse::<u64>()
        .map_err(|_| OntolithError::InvalidState("invalid subject hint"))?;
    Ok(Some(NodeId::new(value)))
}

fn kind_name(kind: QueryKind) -> &'static str {
    match kind {
        QueryKind::Select => "SELECT",
        QueryKind::Construct => "CONSTRUCT",
        QueryKind::Ask => "ASK",
        QueryKind::Describe => "DESCRIBE",
        QueryKind::Update => "UPDATE",
    }
}

fn plan_id_from_query(query: &str) -> QueryPlanId {
    // FNV-1a like hash for deterministic test-friendly plan IDs.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in query.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    QueryPlanId(hash)
}

pub struct InMemoryQueryReadService {
    triple_repo: Arc<dyn TripleRepository>,
}

impl InMemoryQueryReadService {
    pub fn new(triple_repo: Arc<dyn TripleRepository>) -> Self {
        Self { triple_repo }
    }
}

impl QueryReadService for InMemoryQueryReadService {
    fn execute_select_all(
        &self,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started_at = Instant::now();
        let rows = self.triple_repo.all_in_txn(request.txn_id);
        Ok(QueryResultSummary {
            row_count: rows.len(),
            elapsed_ms: started_at.elapsed().as_millis() as u64,
        })
    }

    fn execute_select_by_subject(
        &self,
        request: &QueryRequest,
        subject: NodeId,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started_at = Instant::now();
        let rows = self
            .triple_repo
            .by_subject_in_txn(subject, request.txn_id);
        Ok(QueryResultSummary {
            row_count: rows.len(),
            elapsed_ms: started_at.elapsed().as_millis() as u64,
        })
    }
}

pub struct SimpleQueryPlanner;

impl SimpleQueryPlanner {
    fn kind_from_query(query: &str) -> QueryKind {
        let normalized = query.trim().to_ascii_lowercase();
        if normalized.starts_with("ask") {
            QueryKind::Ask
        } else if normalized.starts_with("construct") {
            QueryKind::Construct
        } else if normalized.starts_with("describe") {
            QueryKind::Describe
        } else if normalized.starts_with("insert")
            || normalized.starts_with("delete")
            || normalized.starts_with("with")
        {
            QueryKind::Update
        } else {
            QueryKind::Select
        }
    }
}

impl QueryPlanner for SimpleQueryPlanner {
    fn plan(&self, request: &QueryRequest) -> Result<QueryPlan, OntolithError> {
        if request.query.0.trim().is_empty() {
            return Err(OntolithError::InvalidState("query text is empty"));
        }

        let kind = Self::kind_from_query(&request.query.0);
        let subject_hint = parse_subject_hint(&request.query.0)?;
        let mut logical_steps = vec!["normalize_query".to_owned()];
        let mut physical_steps = Vec::new();

        match kind {
            QueryKind::Select => {
                logical_steps.push("build_logical_scan".to_owned());
                if subject_hint.is_some() {
                    logical_steps.push("apply_subject_filter".to_owned());
                    physical_steps.push("execute_subject_lookup".to_owned());
                } else {
                    physical_steps.push("execute_scan".to_owned());
                }
            }
            QueryKind::Ask => {
                logical_steps.push("build_logical_ask".to_owned());
                physical_steps.push("execute_boolean_probe".to_owned());
            }
            QueryKind::Construct => {
                logical_steps.push("build_logical_construct".to_owned());
                physical_steps.push("execute_graph_materialization".to_owned());
            }
            QueryKind::Describe => {
                logical_steps.push("build_logical_describe".to_owned());
                physical_steps.push("execute_resource_description".to_owned());
            }
            QueryKind::Update => {
                logical_steps.push("build_logical_update".to_owned());
                physical_steps.push("execute_update".to_owned());
            }
        }

        Ok(QueryPlan {
            id: plan_id_from_query(&request.query.0),
            kind,
            logical_steps,
            physical_steps,
        })
    }
}

pub struct ReadServiceQueryExecutor {
    read_service: Arc<dyn QueryReadService>,
}

impl ReadServiceQueryExecutor {
    pub fn new(read_service: Arc<dyn QueryReadService>) -> Self {
        Self { read_service }
    }
}

impl QueryExecutor for ReadServiceQueryExecutor {
    fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError> {
        if !matches!(plan.kind, QueryKind::Select) {
            return Err(OntolithError::Unsupported(
                kind_name(plan.kind),
            ));
        }

        if let Some(subject) = parse_subject_hint(&request.query.0)? {
            return self
                .read_service
                .execute_select_by_subject(request, subject);
        }

        self.read_service.execute_select_all(request)
    }
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::InMemoryQueryReadService;
    use super::{ReadServiceQueryExecutor, SimpleQueryPlanner};
    use crate::application::{QueryPipeline, QueryPlanner, QueryReadService};
    use crate::domain::{QueryRequest, QueryText};
    use ontolith_core::error::OntolithError;
    use ontolith_core::domain::{Iri, NodeId};
    use ontolith_rdf::domain::{Term, Triple};
    use ontolith_storage::application::{StorageEngine, TripleRepository};
    use ontolith_storage::infrastructure::{InMemoryStorageEngine, InMemoryTripleRepository};
    use ontolith_transaction::domain::TxnId;
    use std::sync::Arc;

    #[test]
    fn read_service_returns_total_rows() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let service = InMemoryQueryReadService::new(Arc::clone(&repo));

        repo.insert(
            TxnId::new(100),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(100))
            .expect("commit must make data visible");

        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o }".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let result = service
            .execute_select_all(&request)
            .expect("query must succeed");

        assert_eq!(result.row_count, 1);
    }

    #[test]
    fn read_service_filters_by_subject() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let service = InMemoryQueryReadService::new(Arc::clone(&repo));
        let target = NodeId::new(7);

        repo.insert(
            TxnId::new(101),
            Triple {
                subject: target,
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o1")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(101))
            .expect("commit must make first write visible");
        repo.insert(
            TxnId::new(102),
            Triple {
                subject: NodeId::new(8),
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o2")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(102))
            .expect("commit must make second write visible");

        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o }".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let result = service
            .execute_select_by_subject(&request, target)
            .expect("query must succeed");

        assert_eq!(result.row_count, 1);
    }

    #[test]
    fn read_service_sees_pending_rows_only_with_same_txn_id() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let service = InMemoryQueryReadService::new(Arc::clone(&repo));
        let txn_id = TxnId::new(200);

        repo.insert(
            txn_id,
            Triple {
                subject: NodeId::new(33),
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o")),
            },
        )
        .expect("insert must succeed");

        let anonymous_request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o }".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let in_txn_request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o }".to_owned()),
            txn_id: Some(txn_id),
            tenant: None,
        };

        let outside_rows = service
            .execute_select_all(&anonymous_request)
            .expect("query must succeed outside txn")
            .row_count;
        let inside_rows = service
            .execute_select_all(&in_txn_request)
            .expect("query must succeed inside txn")
            .row_count;

        assert_eq!(outside_rows, 0);
        assert_eq!(inside_rows, 1);
    }

    #[test]
    fn pipeline_executes_select_with_subject_hint() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let read_service: Arc<dyn QueryReadService> =
            Arc::new(InMemoryQueryReadService::new(Arc::clone(&repo)));
        let planner = SimpleQueryPlanner;
        let executor = ReadServiceQueryExecutor::new(read_service);
        let pipeline = QueryPipeline::new(planner, executor);

        repo.insert(
            TxnId::new(300),
            Triple {
                subject: NodeId::new(100),
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o1")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(300))
            .expect("commit must succeed");

        repo.insert(
            TxnId::new(301),
            Triple {
                subject: NodeId::new(101),
                predicate: Iri::new("urn:test:p"),
                object: Term::Iri(Iri::new("urn:test:o2")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(301))
            .expect("commit must succeed");

        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o } # subject=100".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let result = pipeline.execute(&request).expect("pipeline execute must succeed");
        assert_eq!(result.row_count, 1);
    }

    #[test]
    fn planner_emits_subject_filter_steps_when_hint_present() {
        let planner = SimpleQueryPlanner;
        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o } # subject=42".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let plan = planner.plan(&request).expect("planner must succeed");
        assert!(plan.logical_steps.contains(&"apply_subject_filter".to_owned()));
        assert_eq!(plan.physical_steps, vec!["execute_subject_lookup".to_owned()]);
    }

    #[test]
    fn planner_rejects_invalid_subject_hint() {
        let planner = SimpleQueryPlanner;
        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o } # subject=abc".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let err = planner
            .plan(&request)
            .expect_err("planner should reject invalid subject hint");
        assert_eq!(err, OntolithError::InvalidState("invalid subject hint"));
    }

    #[test]
    fn pipeline_rejects_non_select_queries() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let read_service: Arc<dyn QueryReadService> =
            Arc::new(InMemoryQueryReadService::new(Arc::clone(&repo)));
        let planner = SimpleQueryPlanner;
        let executor = ReadServiceQueryExecutor::new(read_service);
        let pipeline = QueryPipeline::new(planner, executor);

        let request = QueryRequest {
            query: QueryText("ASK { ?s ?p ?o }".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let err = pipeline.execute(&request).expect_err("ASK should be rejected");
        assert_eq!(err, OntolithError::Unsupported("ASK"));
    }

    #[test]
    fn pipeline_rejects_invalid_subject_hint() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> = Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let read_service: Arc<dyn QueryReadService> =
            Arc::new(InMemoryQueryReadService::new(Arc::clone(&repo)));
        let planner = SimpleQueryPlanner;
        let executor = ReadServiceQueryExecutor::new(read_service);
        let pipeline = QueryPipeline::new(planner, executor);

        let request = QueryRequest {
            query: QueryText("SELECT * WHERE { ?s ?p ?o } # subject=".to_owned()),
            txn_id: None,
            tenant: None,
        };

        let err = pipeline
            .execute(&request)
            .expect_err("pipeline should reject invalid subject hint");
        assert_eq!(err, OntolithError::InvalidState("invalid subject hint"));
    }
}
