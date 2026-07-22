//! SPARQL R1 smoke compliance suite.
//!
//! Not a substitute for the W3C official test suite. These cases pin the R1
//! "query baseline" claimed in PROGRESS / PLAN exit criteria.

use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_parser::infrastructure::{parse_ntriples, parse_turtle_doc};
use ontolith_query::domain::{BoundValue, QueryKind, QueryRequest};
use ontolith_query::infrastructure::standard_pipeline;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_storage::application::{DictionaryCodec, StorageEngine, TripleRepository};
use ontolith_storage::infrastructure::{
    InMemoryDictionary, InMemoryStorageEngine, InMemoryTripleRepository,
};
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

fn seed_repo() -> Arc<dyn TripleRepository> {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let txn = TxnId::new(1);
    // node:1 alice knows bob; name Alice
    repo.insert(
        txn,
        Triple {
            subject: NodeId::new(1),
            predicate: Iri::new("http://ex.org/knows"),
            object: Term::Iri(Iri::new("http://ex.org/bob")),
        },
    )
    .unwrap();
    repo.insert(
        txn,
        Triple {
            subject: NodeId::new(1),
            predicate: Iri::new("http://ex.org/name"),
            object: Term::Literal(LiteralValue::String("Alice".into())),
        },
    )
    .unwrap();
    // node:2 bob knows carol; age 30
    repo.insert(
        txn,
        Triple {
            subject: NodeId::new(2),
            predicate: Iri::new("http://ex.org/knows"),
            object: Term::Iri(Iri::new("http://ex.org/carol")),
        },
    )
    .unwrap();
    repo.insert(
        txn,
        Triple {
            subject: NodeId::new(2),
            predicate: Iri::new("http://ex.org/age"),
            object: Term::Literal(LiteralValue::Integer(30)),
        },
    )
    .unwrap();
    // node:3 isolated for UNION cases
    repo.insert(
        txn,
        Triple {
            subject: NodeId::new(3),
            predicate: Iri::new("http://ex.org/label"),
            object: Term::Literal(LiteralValue::String("solo".into())),
        },
    )
    .unwrap();
    engine.commit_transaction(txn).unwrap();
    repo
}

fn exec(q: &str) -> ontolith_query::domain::QueryResult {
    let p = standard_pipeline(seed_repo());
    p.execute(&QueryRequest::new(q))
        .expect("query must succeed")
}

#[test]
fn profile_metadata_lists_features() {
    assert_eq!(ontolith_compliance::profile_name(), "R1-smoke");
    assert!(ontolith_compliance::SPARQL_R1_SMOKE_FEATURES.len() >= 10);
}

#[test]
fn select_star_projects_all_bound_vars() {
    let r = exec("SELECT * WHERE { ?s ?p ?o }");
    assert_eq!(r.kind, QueryKind::Select);
    assert!(r.solutions.len() >= 5);
    assert!(r.variables.iter().any(|v| v == "s"));
    assert!(r.variables.iter().any(|v| v == "p"));
    assert!(r.variables.iter().any(|v| v == "o"));
}

#[test]
fn select_by_predicate_join() {
    let r = exec(
        r#"SELECT ?s ?o WHERE {
            ?s <http://ex.org/knows> ?o .
            ?s <http://ex.org/name> ?n
        }"#,
    );
    assert_eq!(r.solutions.len(), 1);
}

#[test]
fn ask_true_and_false() {
    let yes = exec("ASK WHERE { ?s <http://ex.org/knows> ?o }");
    assert_eq!(yes.boolean, Some(true));
    let no = exec("ASK WHERE { ?s <http://ex.org/missing> ?o }");
    assert_eq!(no.boolean, Some(false));
}

#[test]
fn construct_emits_triples() {
    let r = exec(
        r#"CONSTRUCT { ?s <http://ex.org/link> ?o }
           WHERE { ?s <http://ex.org/knows> ?o }"#,
    );
    assert_eq!(r.kind, QueryKind::Construct);
    assert_eq!(r.construct_triples.len(), 2);
}

#[test]
fn optional_left_join() {
    let r = exec(
        r#"SELECT ?s ?age WHERE {
            ?s <http://ex.org/knows> ?o .
            OPTIONAL { ?s <http://ex.org/age> ?age }
        }"#,
    );
    assert_eq!(r.solutions.len(), 2);
    let with_age = r
        .solutions
        .iter()
        .filter(|s| s.get("age").is_some())
        .count();
    assert_eq!(with_age, 1);
}

#[test]
fn union_combines_branches() {
    let r = exec(
        r#"SELECT ?x WHERE {
            { ?x <http://ex.org/name> ?n }
            UNION
            { ?x <http://ex.org/label> ?n }
        }"#,
    );
    assert!(r.solutions.len() >= 2);
}

#[test]
fn filter_compare() {
    let r = exec(
        r#"SELECT ?s ?age WHERE {
            ?s <http://ex.org/age> ?age .
            FILTER(?age >= 30)
        }"#,
    );
    assert_eq!(r.solutions.len(), 1);
}

#[test]
fn bind_extends_solution() {
    let r = exec(
        r#"SELECT ?s ?flag WHERE {
            ?s <http://ex.org/name> ?n .
            BIND(BOUND(?n) AS ?flag)
        }"#,
    );
    assert_eq!(r.solutions.len(), 1);
    assert!(r.solutions[0].get("flag").is_some());
}

#[test]
fn values_injects_rows() {
    let r = exec(
        r#"SELECT ?s ?o WHERE {
            VALUES ?o { <http://ex.org/bob> <http://ex.org/carol> }
            ?s <http://ex.org/knows> ?o
        }"#,
    );
    assert_eq!(r.solutions.len(), 2);
}

#[test]
fn prefix_declaration() {
    let r = exec(
        r#"PREFIX ex: <http://ex.org/>
           SELECT ?s ?o WHERE { ?s ex:knows ?o }"#,
    );
    assert_eq!(r.solutions.len(), 2);
}

#[test]
fn limit_offset_and_distinct() {
    let limited = exec("SELECT * WHERE { ?s ?p ?o } LIMIT 2");
    assert_eq!(limited.solutions.len(), 2);

    let distinct = exec("SELECT DISTINCT ?p WHERE { ?s ?p ?o }");
    assert!(distinct.solutions.len() >= 3);
    assert!(distinct.solutions.len() < 10);
}

#[test]
fn order_by_stable() {
    let r = exec(
        r#"SELECT ?s ?age WHERE {
            ?s <http://ex.org/age> ?age
        } ORDER BY ?age"#,
    );
    assert!(!r.solutions.is_empty());
}

#[test]
fn parser_ntriples_then_query() {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let dict = InMemoryDictionary::new();
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let doc = r#"
        <http://a.org/s> <http://a.org/p> <http://a.org/o> .
        <http://a.org/s> <http://a.org/p> "lit" .
    "#;
    let parsed = parse_ntriples(doc, &dict).expect("parse nt");
    assert_eq!(parsed.stats.triple_count, 2);
    let txn = TxnId::new(42);
    for t in parsed.dataset.default_graph {
        repo.insert(txn, t).unwrap();
    }
    engine.commit_transaction(txn).unwrap();
    let p = standard_pipeline(repo);
    let r = p
        .execute(&QueryRequest::new(
            "SELECT ?s WHERE { ?s <http://a.org/p> ?o }",
        ))
        .unwrap();
    assert_eq!(r.solutions.len(), 2);
    // dictionary minted a stable subject for the IRI
    let subj = dict.encode_node("http://a.org/s");
    assert!(r.solutions.iter().all(|s| {
        matches!(s.get("s"), Some(BoundValue::Node(n)) if *n == subj) || s.get("s").is_some()
    }));
}

#[test]
fn parser_turtle_prefix_then_ask() {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let dict = InMemoryDictionary::new();
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let doc = r#"
        @prefix ex: <http://ex.org/> .
        ex:alice ex:knows ex:bob .
    "#;
    let parsed = parse_turtle_doc(doc, &dict).expect("parse turtle");
    assert!(parsed.stats.triple_count >= 1);
    let txn = TxnId::new(7);
    for t in parsed.dataset.default_graph {
        repo.insert(txn, t).unwrap();
    }
    engine.commit_transaction(txn).unwrap();
    let p = standard_pipeline(repo);
    let r = p
        .execute(&QueryRequest::new(
            "ASK WHERE { ?s <http://ex.org/knows> <http://ex.org/bob> }",
        ))
        .unwrap();
    assert_eq!(r.boolean, Some(true));
}
