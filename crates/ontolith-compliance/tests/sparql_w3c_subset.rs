//! SPARQL W3C-inspired subset harness.
//!
//! This suite is a step between R1 smoke tests and a full manifest-driven W3C
//! integration. It classifies cases into must-pass, known-gap, and unsupported.

use ontolith_parser::infrastructure::{parse_ntriples, parse_turtle_doc};
use ontolith_query::domain::{QueryKind, QueryRequest};
use ontolith_query::infrastructure::standard_pipeline_with_dictionary;
use ontolith_storage::application::{StorageEngine, TripleRepository};
use ontolith_storage::infrastructure::{
    InMemoryDictionary, InMemoryStorageEngine, InMemoryTripleRepository,
};
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseClass {
    MustPass,
    KnownGap,
    Unsupported,
}

impl CaseClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::MustPass => "must-pass",
            Self::KnownGap => "known-gap",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DatasetFormat {
    NTriples,
    Turtle,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedOutcome {
    SelectRows {
        rows: usize,
        vars: &'static [&'static str],
    },
    Ask(bool),
    ConstructRows(usize),
}

#[derive(Debug, Clone, Copy)]
struct W3cCase {
    id: &'static str,
    source: &'static str,
    feature: &'static str,
    class: CaseClass,
    reason: &'static str,
    format: DatasetFormat,
    dataset: &'static str,
    query: &'static str,
    expected: Option<ExpectedOutcome>,
}

#[derive(Default)]
struct Summary {
    total: usize,
    executed: usize,
    skipped: usize,
    must_pass_ok: usize,
    must_pass_failed: usize,
    xfail: usize,
    xpass: usize,
}

#[test]
fn w3c_subset_profile() {
    let strict = env_flag("ONTOLITH_W3C_SUBSET_STRICT");
    let cases = cases();
    let mut summary = Summary {
        total: cases.len(),
        ..Summary::default()
    };
    let mut failures = Vec::new();

    for case in &cases {
        println!(
            "[W3C subset] id={} class={} feature={} source={}",
            case.id,
            case.class.as_str(),
            case.feature,
            case.source
        );

        match case.class {
            CaseClass::Unsupported => {
                summary.skipped += 1;
                println!("  -> SKIP: {}", case.reason);
            }
            CaseClass::KnownGap => {
                summary.executed += 1;
                match run_case(case) {
                    Ok(()) => {
                        summary.xpass += 1;
                        println!("  -> XPASS: known gap now passes (consider reclassifying)");
                    }
                    Err(err) => {
                        summary.xfail += 1;
                        println!("  -> XFAIL: {}", err);
                    }
                }
            }
            CaseClass::MustPass => {
                summary.executed += 1;
                match run_case(case) {
                    Ok(()) => {
                        summary.must_pass_ok += 1;
                        println!("  -> PASS");
                    }
                    Err(err) => {
                        summary.must_pass_failed += 1;
                        failures.push(format!("{}: {}", case.id, err));
                        println!("  -> FAIL: {}", err);
                    }
                }
            }
        }
    }

    println!(
        "[W3C subset summary] total={} executed={} skipped={} must-pass(ok/fail)={}/{} xfail={} xpass={} strict={}",
        summary.total,
        summary.executed,
        summary.skipped,
        summary.must_pass_ok,
        summary.must_pass_failed,
        summary.xfail,
        summary.xpass,
        strict
    );

    if !failures.is_empty() {
        println!("[W3C subset failures]");
        for f in &failures {
            println!("  - {}", f);
        }
    }

    assert_eq!(summary.must_pass_failed, 0, "must-pass cases failed");

    if strict {
        assert_eq!(
            summary.xfail, 0,
            "strict mode requires zero known-gap failures"
        );
        assert_eq!(
            summary.skipped, 0,
            "strict mode requires zero skipped cases"
        );
    }
}

fn run_case(case: &W3cCase) -> Result<(), String> {
    let (repo, dict) = load_repo(case.format, case.dataset)?;
    let pipeline = standard_pipeline_with_dictionary(repo, dict);
    let result = pipeline
        .execute(&QueryRequest::new(case.query))
        .map_err(|e| format!("query execution error: {e:?}"))?;

    match case.expected {
        Some(ExpectedOutcome::SelectRows { rows, vars }) => {
            if result.kind != QueryKind::Select {
                return Err(format!("expected SELECT, got {:?}", result.kind));
            }
            if result.solutions.len() != rows {
                return Err(format!(
                    "expected {} rows, got {}",
                    rows,
                    result.solutions.len()
                ));
            }
            for required in vars {
                if !result.variables.iter().any(|v| v == required) {
                    return Err(format!("missing projected variable '?{}'", required));
                }
            }
            Ok(())
        }
        Some(ExpectedOutcome::Ask(expect)) => {
            if result.kind != QueryKind::Ask {
                return Err(format!("expected ASK, got {:?}", result.kind));
            }
            if result.boolean != Some(expect) {
                return Err(format!("expected ASK={expect}, got {:?}", result.boolean));
            }
            Ok(())
        }
        Some(ExpectedOutcome::ConstructRows(rows)) => {
            if result.kind != QueryKind::Construct {
                return Err(format!("expected CONSTRUCT, got {:?}", result.kind));
            }
            if result.construct_triples.len() != rows {
                return Err(format!(
                    "expected {} constructed triples, got {}",
                    rows,
                    result.construct_triples.len()
                ));
            }
            Ok(())
        }
        None => Ok(()),
    }
}

fn load_repo(
    format: DatasetFormat,
    dataset: &str,
) -> Result<(Arc<dyn TripleRepository>, Arc<InMemoryDictionary>), String> {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let dict = Arc::new(InMemoryDictionary::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));

    let parsed =
        match format {
            DatasetFormat::NTriples => parse_ntriples(dataset, dict.as_ref())
                .map_err(|e| format!("parse ntriples failed: {e:?}"))?,
            DatasetFormat::Turtle => parse_turtle_doc(dataset, dict.as_ref())
                .map_err(|e| format!("parse turtle failed: {e:?}"))?,
        };

    let txn = TxnId::new(1);
    for triple in parsed.dataset.default_graph {
        repo.insert(txn, triple)
            .map_err(|e| format!("repository insert failed: {e:?}"))?;
    }
    engine
        .commit_transaction(txn)
        .map_err(|e| format!("storage commit failed: {e:?}"))?;

    Ok((repo, dict))
}

fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn cases() -> Vec<W3cCase> {
    vec![
        W3cCase {
            id: "w3c-select-basic",
            source: "W3C SPARQL 1.1 Query tests (derived basic SELECT)",
            feature: "SELECT BGP",
            class: CaseClass::MustPass,
            reason: "core SELECT baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/select_basic.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["s"],
            }),
        },
        W3cCase {
            id: "w3c-ask-basic",
            source: "W3C SPARQL 1.1 Query tests (derived basic ASK)",
            feature: "ASK",
            class: CaseClass::MustPass,
            reason: "core ASK baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/ask_basic.rq"),
            expected: Some(ExpectedOutcome::Ask(true)),
        },
        W3cCase {
            id: "w3c-construct-basic",
            source: "W3C SPARQL 1.1 Query tests (derived basic CONSTRUCT)",
            feature: "CONSTRUCT",
            class: CaseClass::MustPass,
            reason: "core CONSTRUCT baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/construct_basic.rq"),
            expected: Some(ExpectedOutcome::ConstructRows(2)),
        },
        W3cCase {
            id: "w3c-optional-basic",
            source: "W3C SPARQL 1.1 Query tests (derived optional)",
            feature: "OPTIONAL",
            class: CaseClass::MustPass,
            reason: "left join support",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/optional_basic.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["s", "name"],
            }),
        },
        W3cCase {
            id: "w3c-union-basic",
            source: "W3C SPARQL 1.1 Query tests (derived union)",
            feature: "UNION",
            class: CaseClass::MustPass,
            reason: "set union support",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/union_basic.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 3,
                vars: &["x"],
            }),
        },
        W3cCase {
            id: "w3c-filter-bound",
            source: "W3C SPARQL 1.1 Query tests (derived filter bound)",
            feature: "FILTER BOUND",
            class: CaseClass::MustPass,
            reason: "filter predicate baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/filter_bound.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["s", "n"],
            }),
        },
        W3cCase {
            id: "w3c-bind-bound",
            source: "W3C SPARQL 1.1 Query tests (derived bind)",
            feature: "BIND",
            class: CaseClass::MustPass,
            reason: "bind expression baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/bind_bound.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["s", "hasAge"],
            }),
        },
        W3cCase {
            id: "w3c-values-basic",
            source: "W3C SPARQL 1.1 Query tests (derived values)",
            feature: "VALUES",
            class: CaseClass::MustPass,
            reason: "inline binding baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/values_basic.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["s", "o"],
            }),
        },
        W3cCase {
            id: "w3c-distinct-order-limit",
            source: "W3C SPARQL 1.1 Query tests (derived projection modifiers)",
            feature: "DISTINCT ORDER BY LIMIT",
            class: CaseClass::MustPass,
            reason: "solution modifier baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/distinct_order_limit.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["p"],
            }),
        },
        W3cCase {
            id: "w3c-prefix-ask-turtle",
            source: "W3C SPARQL 1.1 Query tests (derived prefix + turtle)",
            feature: "PREFIX + ASK",
            class: CaseClass::MustPass,
            reason: "prefix and turtle ingest baseline",
            format: DatasetFormat::Turtle,
            dataset: include_str!("w3c/data/basic.ttl"),
            query: include_str!("w3c/queries/prefix_ask_turtle.rq"),
            expected: Some(ExpectedOutcome::Ask(true)),
        },
        W3cCase {
            id: "w3c-subquery-gap",
            source: "W3C SPARQL 1.1 Query tests (subquery)",
            feature: "Subquery",
            class: CaseClass::MustPass,
            reason: "subquery baseline (nested SELECT + LIMIT)",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/subquery_gap.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 1,
                vars: &["s"],
            }),
        },
        W3cCase {
            id: "w3c-aggregate-gap",
            source: "W3C SPARQL 1.1 Query tests (derived aggregate)",
            feature: "Aggregate COUNT",
            class: CaseClass::MustPass,
            reason: "COUNT aggregate baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/aggregate_gap.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 1,
                vars: &["c"],
            }),
        },
        W3cCase {
            id: "w3c-property-path-sequence",
            source: "W3C SPARQL 1.1 Query tests (property path)",
            feature: "Property path sequence",
            class: CaseClass::MustPass,
            reason: "property path sequence baseline (iri/iri)",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/property_path_unsupported.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 1,
                vars: &["s"],
            }),
        },
        W3cCase {
            id: "w3c-property-path-plus",
            source: "W3C SPARQL 1.1 Query tests (property path)",
            feature: "Property path +",
            class: CaseClass::MustPass,
            reason: "one-or-more transitive closure baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/property_path_plus.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["o"],
            }),
        },
        W3cCase {
            id: "w3c-property-path-star",
            source: "W3C SPARQL 1.1 Query tests (property path)",
            feature: "Property path *",
            class: CaseClass::MustPass,
            reason: "zero-or-more closure includes reflexive reachability",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/property_path_star.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 3,
                vars: &["o"],
            }),
        },
        W3cCase {
            id: "w3c-property-path-alternative",
            source: "W3C SPARQL 1.1 Query tests (property path)",
            feature: "Property path |",
            class: CaseClass::MustPass,
            reason: "alternation baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/property_path_alternative.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 2,
                vars: &["x"],
            }),
        },
        W3cCase {
            id: "w3c-property-path-inverse",
            source: "W3C SPARQL 1.1 Query tests (property path)",
            feature: "Property path ^",
            class: CaseClass::MustPass,
            reason: "inverse predicate baseline",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/property_path_inverse.rq"),
            expected: Some(ExpectedOutcome::SelectRows {
                rows: 1,
                vars: &["s"],
            }),
        },
        W3cCase {
            id: "w3c-update-unsupported",
            source: "W3C SPARQL 1.1 Update tests",
            feature: "SPARQL Update",
            class: CaseClass::Unsupported,
            reason: "update operations are not yet implemented",
            format: DatasetFormat::NTriples,
            dataset: include_str!("w3c/data/basic.nt"),
            query: include_str!("w3c/queries/update_unsupported.ru"),
            expected: None,
        },
    ]
}
