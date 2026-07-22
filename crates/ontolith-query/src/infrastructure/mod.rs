//! Query infrastructure: SPARQL parse, optimize, execute (L3 full).

mod execute;
mod optimize;
mod sparql_parse;

// Keep legacy name available for external references.
#[allow(dead_code)]
mod sparql_mvp_legacy {
    // Intentionally empty shim — full engine replaces sparql_mvp.
}

use crate::application::{QueryExecutor, QueryPlanner, QueryReadService};
#[cfg(test)]
use crate::domain::QueryResultSummary;
use crate::domain::{QueryPlan, QueryRequest, QueryResult};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::TripleRepository;
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

pub use execute::AlgebraExecutor;
pub use optimize::RuleBasedOptimizer;
pub use sparql_parse::{parse_subject_hint, plan_query};

/// Storage-backed read service using SPO/POS/OSP indexes.
pub struct InMemoryQueryReadService {
    triple_repo: Arc<dyn TripleRepository>,
}

impl InMemoryQueryReadService {
    pub fn new(triple_repo: Arc<dyn TripleRepository>) -> Self {
        Self { triple_repo }
    }
}

impl QueryReadService for InMemoryQueryReadService {
    fn all_triples(&self, txn_id: Option<TxnId>) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.all_in_txn(txn_id))
    }

    fn by_subject(
        &self,
        subject: NodeId,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_subject_in_txn(subject, txn_id))
    }

    fn by_predicate(
        &self,
        predicate: &Iri,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_predicate_in_txn(predicate, txn_id))
    }

    fn by_object(
        &self,
        object: &Term,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self.triple_repo.by_object_in_txn(object, txn_id))
    }

    fn matching(
        &self,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        Ok(self
            .triple_repo
            .matching_in_txn(subject, predicate, object, txn_id))
    }
}

/// Full SPARQL planner.
#[derive(Debug, Default, Clone, Copy)]
pub struct SimpleQueryPlanner;

impl QueryPlanner for SimpleQueryPlanner {
    fn plan(&self, request: &QueryRequest) -> Result<QueryPlan, OntolithError> {
        plan_query(request)
    }
}

/// Executor adapter implementing [`QueryExecutor`].
pub struct ReadServiceQueryExecutor {
    inner: AlgebraExecutor,
}

impl ReadServiceQueryExecutor {
    pub fn new(read_service: Arc<dyn QueryReadService>) -> Self {
        Self {
            inner: AlgebraExecutor::new(read_service),
        }
    }
}

impl QueryExecutor for ReadServiceQueryExecutor {
    fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResult, OntolithError> {
        self.inner.execute(plan, request)
    }
}

/// Build the standard L3 pipeline: parse → rule optimize → execute.
pub fn standard_pipeline(
    repo: Arc<dyn TripleRepository>,
) -> crate::application::QueryPipeline<
    SimpleQueryPlanner,
    RuleBasedOptimizer,
    ReadServiceQueryExecutor,
> {
    let read: Arc<dyn QueryReadService> = Arc::new(InMemoryQueryReadService::new(repo));
    crate::application::QueryPipeline::new(
        SimpleQueryPlanner,
        RuleBasedOptimizer,
        ReadServiceQueryExecutor::new(read),
    )
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::QueryPipeline;
    use crate::domain::{Algebra, BoundValue, QueryRequest, TermPattern};
    use ontolith_core::domain::{Iri, LiteralValue, NodeId};
    use ontolith_rdf::domain::{Term, Triple};
    use ontolith_storage::application::{StorageEngine, TripleRepository};
    use ontolith_storage::infrastructure::{InMemoryStorageEngine, InMemoryTripleRepository};
    use ontolith_transaction::domain::TxnId;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn seed() -> (Arc<InMemoryStorageEngine>, Arc<dyn TripleRepository>) {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        // alice knows bob
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/bob")),
            },
        )
        .unwrap();
        // alice name "Alice"
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(1),
                predicate: Iri::new("http://ex.org/name"),
                object: Term::Literal(LiteralValue::String("Alice".into())),
            },
        )
        .unwrap();
        // bob knows carol
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(2),
                predicate: Iri::new("http://ex.org/knows"),
                object: Term::Iri(Iri::new("http://ex.org/carol")),
            },
        )
        .unwrap();
        // bob age 30
        repo.insert(
            TxnId::new(1),
            Triple {
                subject: NodeId::new(2),
                predicate: Iri::new("http://ex.org/age"),
                object: Term::Literal(LiteralValue::Integer(30)),
            },
        )
        .unwrap();
        engine.commit_transaction(TxnId::new(1)).unwrap();
        // encode dictionary-style ids used in SPARQL via node:N and absolute IRIs on predicates/objects
        (engine, repo)
    }

    fn pipeline(
        repo: Arc<dyn TripleRepository>,
    ) -> QueryPipeline<SimpleQueryPlanner, RuleBasedOptimizer, ReadServiceQueryExecutor> {
        standard_pipeline(repo)
    }

    #[test]
    fn select_star_returns_all_triples_as_solutions() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert_eq!(result.solutions.len(), 4);
        assert!(result.variables.contains(&"s".into()));
        assert!(result.variables.contains(&"p".into()));
        assert!(result.variables.contains(&"o".into()));
    }

    #[test]
    fn select_by_predicate_uses_pos() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
        let explain = p
            .explain(&QueryRequest::new(
                "SELECT ?s ?o WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert!(
            explain
                .physical_steps
                .iter()
                .any(|s| s.contains("index_pos") || s.contains("bgp"))
        );
    }

    #[test]
    fn join_two_patterns() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        // node:1 knows ?o . node:1 name ?n
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?o ?n WHERE {
                    node:1 <http://ex.org/knows> ?o .
                    node:1 <http://ex.org/name> ?n
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("n"),
            Some(&BoundValue::Literal(LiteralValue::String("Alice".into())))
        );
    }

    #[test]
    fn optional_left_join() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?age WHERE {
                    ?s <http://ex.org/knows> ?o .
                    OPTIONAL { ?s <http://ex.org/age> ?age }
                }"#,
            ))
            .unwrap();
        // two knows triples; only bob(node:2) has age
        assert_eq!(result.solutions.len(), 2);
        let with_age = result
            .solutions
            .iter()
            .filter(|s| s.get("age").is_some())
            .count();
        assert_eq!(with_age, 1);
    }

    #[test]
    fn filter_bound_and_compare() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?age WHERE {
                    ?s <http://ex.org/age> ?age .
                    FILTER(?age >= 30)
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn union_combines_branches() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?x WHERE {
                    { node:1 <http://ex.org/name> ?x }
                    UNION
                    { node:2 <http://ex.org/age> ?x }
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn bind_extends_solution() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?flag WHERE {
                    ?s <http://ex.org/name> ?n .
                    BIND(BOUND(?n) AS ?flag)
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("flag"),
            Some(&BoundValue::Literal(LiteralValue::Boolean(true)))
        );
    }

    #[test]
    fn values_clause() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"SELECT ?s ?o WHERE {
                    VALUES ?s { node:1 }
                    ?s <http://ex.org/knows> ?o
                }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn ask_true_false() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let yes = p
            .execute(&QueryRequest::new(
                "ASK WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(yes.boolean, Some(true));
        let no = p
            .execute(&QueryRequest::new(
                "ASK WHERE { ?s <http://ex.org/missing> ?o }",
            ))
            .unwrap();
        assert_eq!(no.boolean, Some(false));
    }

    #[test]
    fn construct_builds_triples() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"CONSTRUCT { ?s <http://ex.org/copy> ?o }
                   WHERE { ?s <http://ex.org/knows> ?o }"#,
            ))
            .unwrap();
        assert_eq!(result.construct_triples.len(), 2);
    }

    #[test]
    fn distinct_and_limit_offset() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT DISTINCT ?p WHERE { ?s ?p ?o } ORDER BY ?p LIMIT 1 OFFSET 0",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
    }

    #[test]
    fn explain_contains_optimize_step() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let explain = p
            .explain(&QueryRequest::new(
                "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert!(
            explain
                .logical_steps
                .iter()
                .any(|s| s.starts_with("optimize:"))
        );
        assert!(!explain.algebra_summary.is_empty());
    }

    #[test]
    fn timeout_zero() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_timeout(0))
            .unwrap();
        assert!(result.timed_out);
    }

    #[test]
    fn cancel_flag() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let flag = Arc::new(AtomicBool::new(true));
        let result = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_cancel(flag))
            .unwrap();
        assert!(result.cancelled);
    }

    #[test]
    fn prefix_expansion() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                r#"PREFIX ex: <http://ex.org/>
                   SELECT ?s ?o WHERE { ?s ex:knows ?o }"#,
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2);
    }

    #[test]
    fn legacy_subject_hint_still_works() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT * WHERE { ?s ?p ?o } # subject=1",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 2); // alice has 2 triples
    }

    #[test]
    fn txn_visibility() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo: Arc<dyn TripleRepository> =
            Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
        let p = pipeline(Arc::clone(&repo));
        let txn = TxnId::new(9);
        repo.insert(
            txn,
            Triple {
                subject: NodeId::new(99),
                predicate: Iri::new("http://ex.org/p"),
                object: Term::Iri(Iri::new("http://ex.org/o")),
            },
        )
        .unwrap();
        let outside = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }"))
            .unwrap();
        assert_eq!(outside.solutions.len(), 0);
        let inside = p
            .execute(&QueryRequest::new("SELECT * WHERE { ?s ?p ?o }").with_txn(txn))
            .unwrap();
        assert_eq!(inside.solutions.len(), 1);
    }

    #[test]
    fn summary_compat() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let summary: QueryResultSummary = p
            .execute_summary(&QueryRequest::new(
                "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
            ))
            .unwrap();
        assert_eq!(summary.row_count, 2);
    }

    #[test]
    fn algebra_binds_node_subject() {
        let planner = SimpleQueryPlanner;
        let plan = planner
            .plan(&QueryRequest::new("SELECT * WHERE { node:1 ?p ?o }"))
            .unwrap();
        // after project wrapper
        fn find_bgp(a: &Algebra) -> bool {
            match a {
                Algebra::Bgp(p) => matches!(p[0].subject, TermPattern::Node(_)),
                Algebra::Project { input, .. }
                | Algebra::Slice { input, .. }
                | Algebra::Distinct { input }
                | Algebra::Filter { input, .. } => find_bgp(input),
                Algebra::Join { left, .. } => find_bgp(left),
                _ => false,
            }
        }
        assert!(find_bgp(&plan.algebra));
    }

    #[test]
    fn empty_query_rejected() {
        let planner = SimpleQueryPlanner;
        let err = planner.plan(&QueryRequest::new("   ")).expect_err("empty");
        assert!(matches!(err, OntolithError::InvalidArgument(_)));
    }

    #[test]
    fn unsupported_update() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let err = p
            .execute(&QueryRequest::new("INSERT DATA { <a> <b> <c> }"))
            .expect_err("update");
        assert!(matches!(err, OntolithError::Unsupported(_)));
    }

    #[test]
    fn aggregate_count_without_group_by() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(result.variables, vec!["c".to_string()]);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(4)))
        );
    }

    #[test]
    fn aggregate_count_star_without_group_by() {
        let (_e, repo) = seed();
        let p = pipeline(repo);
        let result = p
            .execute(&QueryRequest::new(
                "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .unwrap();
        assert_eq!(result.solutions.len(), 1);
        assert_eq!(
            result.solutions[0].get("c"),
            Some(&BoundValue::Literal(LiteralValue::Integer(4)))
        );
    }

    #[test]
    fn aggregate_mixed_projection_without_group_by_rejected() {
        let planner = SimpleQueryPlanner;
        let err = planner
            .plan(&QueryRequest::new(
                "SELECT ?s (COUNT(?s) AS ?c) WHERE { ?s ?p ?o }",
            ))
            .expect_err("mixed projection requires group by");
        assert!(matches!(err, OntolithError::Failed(_)));
        assert!(err.message().contains("GROUP BY"));
    }
}
