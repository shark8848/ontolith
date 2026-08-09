//! R2 exit-criteria gate: Explain quality & optimizer stability (PLAN §6 R2).
//!
//! Gate assertions:
//! 1. completeness — every query class (BGP / join / filter / union / left
//!    join / distinct / aggregate / construct / ask / path / graph) exposes
//!    logical steps, physical steps, an algebra summary and cost estimates
//!    (`estimated_rows` / `pattern_costs`);
//! 2. cost sanity — per-pattern `selectivity` ∈ (0, 1], `estimated_rows` ≥ 1,
//!    and the cost optimizer orders the most selective BGP pattern first;
//! 3. semantic preservation — cost-optimized execution returns exactly the
//!    same result set as the rule-based pipeline (optimizer rewrites never
//!    change the answer);
//! 4. stability — repeated `explain()` calls are identical (no nondeterminism
//!    in planning, ordering or cost estimation).

use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_query::domain::{QueryKind, QueryRequest, QueryResult};
use ontolith_query::infrastructure::{cost_pipeline, standard_pipeline};
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::{StorageEngine, TripleRepository};
use ontolith_storage::infrastructure::{InMemoryStorageEngine, InMemoryTripleRepository};
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

const KNOWS: &str = "http://ex.org/knows";
const AGE: &str = "http://ex.org/age";
const LABEL: &str = "http://ex.org/label";

fn seed_repo() -> (Arc<dyn TripleRepository>, Arc<dyn StorageEngine>) {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let txn = TxnId::new(1);
    // 100 knows edges, all to one repeated object so the `<age> 30` pattern is
    // strictly more selective than `?s <knows> ?o` under the uniform heuristic.
    for i in 0..100 {
        repo.insert(
            txn,
            Triple::new(
                NodeId::new(i),
                Iri::new(KNOWS),
                Term::Iri(Iri::new("http://ex.org/target")),
            ),
        )
        .unwrap();
    }
    repo.insert(
        txn,
        Triple::new(
            NodeId::new(0),
            Iri::new(AGE),
            Term::Literal(LiteralValue::Integer(30)),
        ),
    )
    .unwrap();
    repo.insert(
        txn,
        Triple::new(
            NodeId::new(1),
            Iri::new(LABEL),
            Term::Literal(LiteralValue::String("x".into())),
        ),
    )
    .unwrap();
    engine.commit_transaction(txn).unwrap();
    (repo, engine)
}

const CORPUS: &[(&str, QueryKind)] = &[
    (
        "SELECT * WHERE { ?s <http://ex.org/knows> ?o }",
        QueryKind::Select,
    ),
    (
        "SELECT * WHERE { ?s <http://ex.org/knows> ?o . ?o <http://ex.org/knows> ?z }",
        QueryKind::Select,
    ),
    (
        "SELECT * WHERE { ?s <http://ex.org/age> ?a FILTER(?a > 18) }",
        QueryKind::Select,
    ),
    (
        "SELECT * WHERE { { ?s <http://ex.org/knows> ?o } UNION { ?s <http://ex.org/label> ?l } }",
        QueryKind::Select,
    ),
    (
        "SELECT * WHERE { ?s <http://ex.org/knows>+ ?o }",
        QueryKind::Select,
    ),
    (
        "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }",
        QueryKind::Select,
    ),
    (
        "CONSTRUCT { ?s <http://ex.org/sees> ?o } WHERE { ?s <http://ex.org/knows> ?o }",
        QueryKind::Construct,
    ),
    ("ASK { ?s <http://ex.org/knows> ?o }", QueryKind::Ask),
    (
        "SELECT * WHERE { ?s <http://ex.org/knows> ?o OPTIONAL { ?o <http://ex.org/age> ?a } }",
        QueryKind::Select,
    ),
    ("SELECT DISTINCT ?p WHERE { ?s ?p ?o }", QueryKind::Select),
    (
        "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }",
        QueryKind::Select,
    ),
];

fn canonical_solutions(result: &QueryResult) -> Vec<String> {
    let mut rows: Vec<String> = result.solutions.iter().map(|s| format!("{s:?}")).collect();
    rows.sort();
    rows
}

fn assert_same_result(q: &str, standard: &QueryResult, cost: &QueryResult) {
    match standard.kind {
        QueryKind::Ask => assert_eq!(standard.boolean, cost.boolean, "{q}: ASK mismatch"),
        QueryKind::Construct => {
            let mut a: Vec<String> = standard
                .construct_triples
                .iter()
                .map(|t| format!("{t:?}"))
                .collect();
            let mut b: Vec<String> = cost
                .construct_triples
                .iter()
                .map(|t| format!("{t:?}"))
                .collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{q}: CONSTRUCT mismatch");
        }
        QueryKind::Select => {
            assert_eq!(
                canonical_solutions(standard),
                canonical_solutions(cost),
                "{q}: SELECT mismatch"
            );
        }
        QueryKind::Describe | QueryKind::Update => {
            panic!("corpus must not contain describe/update queries")
        }
    }
}

#[test]
fn explain_completeness_across_query_classes() {
    let (repo, engine) = seed_repo();
    let pipeline = cost_pipeline(repo, Arc::clone(&engine));
    for (query, kind) in CORPUS {
        let explain = pipeline
            .explain(&QueryRequest::new(*query))
            .unwrap_or_else(|e| panic!("{query}: explain failed: {e}"));
        assert!(
            !explain.logical_steps.is_empty(),
            "{query}: logical steps empty"
        );
        assert!(
            !explain.physical_steps.is_empty(),
            "{query}: physical steps empty"
        );
        assert!(
            !explain.algebra_summary.is_empty(),
            "{query}: algebra summary empty"
        );
        assert_eq!(explain.kind, *kind, "{query}: kind mismatch");
        assert!(
            explain.estimated_rows.is_some(),
            "{query}: estimated_rows missing"
        );
        for cost in &explain.pattern_costs {
            assert!(
                cost.selectivity > 0.0 && cost.selectivity <= 1.0,
                "{query}: selectivity {} out of range",
                cost.selectivity
            );
            assert!(
                cost.estimated_rows >= 1,
                "{query}: estimated_rows {} < 1",
                cost.estimated_rows
            );
            assert!(!cost.pattern.is_empty(), "{query}: empty pattern signature");
        }
    }
}

#[test]
fn path_explain_carries_uniform_worst_case_cost() {
    let (repo, engine) = seed_repo();
    let pipeline = cost_pipeline(repo, engine);
    let explain = pipeline
        .explain(&QueryRequest::new(
            "SELECT * WHERE { ?s <http://ex.org/knows>+ ?o }",
        ))
        .expect("path explain must succeed");
    assert_eq!(
        explain.pattern_costs.len(),
        1,
        "path must carry exactly one cost estimate"
    );
    assert_eq!(
        explain.pattern_costs[0].selectivity, 1.0,
        "path = uniform worst case"
    );
    assert_eq!(
        explain.pattern_costs[0].estimated_rows, 102,
        "path rows = total triples"
    );
    assert_eq!(explain.estimated_rows, Some(102));
}

#[test]
fn cost_optimizer_orders_most_selective_pattern_first() {
    let (repo, engine) = seed_repo();
    let pipeline = cost_pipeline(repo, engine);
    let explain = pipeline
        .explain(&QueryRequest::new(
            "SELECT * WHERE { ?s <http://ex.org/age> 30 . ?s <http://ex.org/knows> ?o }",
        ))
        .expect("explain must succeed");
    assert_eq!(explain.pattern_costs.len(), 2);
    let age = &explain.pattern_costs[0];
    let knows = &explain.pattern_costs[1];
    assert!(
        age.pattern.contains("age"),
        "first pattern should be age, got {}",
        age.pattern
    );
    assert!(
        knows.pattern.contains("knows"),
        "second pattern should be knows, got {}",
        knows.pattern
    );
    assert!(
        age.estimated_rows <= knows.estimated_rows,
        "age rows {} should be <= knows rows {}",
        age.estimated_rows,
        knows.estimated_rows
    );
}

#[test]
fn cost_optimization_preserves_semantics() {
    let (repo, engine) = seed_repo();
    let cost = cost_pipeline(Arc::clone(&repo), Arc::clone(&engine));
    let standard = standard_pipeline(repo);
    for (query, _) in CORPUS {
        let req = QueryRequest::new(*query);
        let std_res = standard
            .execute(&req)
            .unwrap_or_else(|e| panic!("{query}: standard failed: {e}"));
        let cost_res = cost
            .execute(&req)
            .unwrap_or_else(|e| panic!("{query}: cost failed: {e}"));
        assert_same_result(query, &std_res, &cost_res);
    }
}

#[test]
fn explain_is_stable_across_calls() {
    let (repo, engine) = seed_repo();
    let pipeline = cost_pipeline(repo, engine);
    for (query, _) in CORPUS {
        let req = QueryRequest::new(*query);
        let a = pipeline
            .explain(&req)
            .unwrap_or_else(|e| panic!("{query}: explain failed: {e}"));
        let b = pipeline
            .explain(&req)
            .unwrap_or_else(|e| panic!("{query}: explain failed: {e}"));
        assert_eq!(a.kind, b.kind, "{query}: kind drift");
        assert_eq!(
            a.logical_steps, b.logical_steps,
            "{query}: logical steps drift"
        );
        assert_eq!(
            a.physical_steps, b.physical_steps,
            "{query}: physical steps drift"
        );
        assert_eq!(
            a.algebra_summary, b.algebra_summary,
            "{query}: algebra drift"
        );
        assert_eq!(
            a.estimated_rows, b.estimated_rows,
            "{query}: estimated_rows drift"
        );
        assert_eq!(
            a.pattern_costs, b.pattern_costs,
            "{query}: pattern_costs drift"
        );
    }
}
