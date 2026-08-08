//! R2 exit-criteria gate: reasoner correctness & performance guardrails
//! (PLAN §6 R2).
//!
//! Correctness: OWL 2 RL scenarios pin expected inferences (closure counts and
//! specific derived triples) plus inconsistency detection.
//! Guardrails: the wall-clock budget and iteration cap actually bound runaway
//! materialization (timed-out / partial closure), and a large closure
//! completes within the performance budget.

use ontolith_core::domain::Iri;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_reasoner::application::Reasoner;
use ontolith_reasoner::domain::{InferenceMode, ReasoningReport, ReasoningTask};
use ontolith_reasoner::infrastructure::ForwardChainReasoner;
use ontolith_storage::application::DictionaryCodec;
use ontolith_storage::infrastructure::InMemoryDictionary;
use std::time::Instant;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

fn t(s: &str, p: &str, o: &str, dict: &InMemoryDictionary) -> Triple {
    Triple::new(dict.encode_node(s), Iri::new(p), Term::Iri(Iri::new(o)))
}

/// RDF list `start` → members via first/rest, blank-node encoding.
fn rdf_list(dict: &InMemoryDictionary, start: &str, members: &[&str]) -> Vec<Triple> {
    let prefix = start.trim_start_matches("_:");
    let mut out = Vec::new();
    for (i, m) in members.iter().enumerate() {
        let node = if i == 0 {
            start.to_string()
        } else {
            format!("_:{prefix}{i}")
        };
        out.push(t(&node, RDF_FIRST, m, dict));
        let rest = if i + 1 == members.len() {
            RDF_NIL.to_string()
        } else {
            format!("_:{prefix}{}", i + 1)
        };
        out.push(t(&node, RDF_REST, &rest, dict));
    }
    out
}

fn task(mode: InferenceMode, max_iterations: u32, max_elapsed_ms: Option<u64>) -> ReasoningTask {
    ReasoningTask {
        plan_id: None,
        mode,
        max_iterations,
        max_elapsed_ms,
    }
}

fn materialize(
    dict: &InMemoryDictionary,
    input: &[Triple],
    cfg: ReasoningTask,
) -> (Vec<Triple>, ReasoningReport) {
    let reasoner = ForwardChainReasoner::new();
    let outcome = reasoner
        .materialize(dict, &cfg, input)
        .expect("materialize must succeed");
    (outcome.triples, outcome.report)
}

fn assert_has(outcome: &[Triple], dict: &InMemoryDictionary, s: &str, p: &str, o: &str) {
    assert!(
        outcome.iter().any(|tr| {
            tr.subject == dict.encode_node(s)
                && tr.predicate.as_str() == p
                && tr.object == Term::Iri(Iri::new(o))
        }),
        "missing derived triple {s} {p} {o}"
    );
}

fn subclass_chain(n: usize) -> (InMemoryDictionary, Vec<Triple>, usize) {
    let dict = InMemoryDictionary::new();
    let mut input = Vec::new();
    for i in 1..n {
        input.push(t(
            &format!("urn:n{i}"),
            &format!("{RDFS}subClassOf"),
            &format!("urn:n{}", i + 1),
            &dict,
        ));
    }
    let edges = n - 1;
    let closure = n * (n - 1) / 2; // C(n,2) transitive closure
    (dict, input, closure - edges)
}

#[test]
fn ow2_rl_correctness_core() {
    let dict = InMemoryDictionary::new();
    let mut input = vec![
        // subclass transitivity: A ⊑ B ⊑ C ⊑ D
        t("urn:A", &format!("{RDFS}subClassOf"), "urn:B", &dict),
        t("urn:B", &format!("{RDFS}subClassOf"), "urn:C", &dict),
        t("urn:C", &format!("{RDFS}subClassOf"), "urn:D", &dict),
        // domain/range typing
        t("urn:p", &format!("{RDFS}domain"), "urn:C", &dict),
        t("urn:p", &format!("{RDFS}range"), "urn:D", &dict),
        t("urn:x", "urn:p", "urn:y", &dict),
        // symmetric + transitive property r
        t("urn:r", RDF_TYPE, &format!("{OWL}SymmetricProperty"), &dict),
        t("urn:r", RDF_TYPE, &format!("{OWL}TransitiveProperty"), &dict),
        t("urn:a", "urn:r", "urn:b", &dict),
        t("urn:b", "urn:r", "urn:c", &dict),
        // inverseOf
        t("urn:p", &format!("{OWL}inverseOf"), "urn:q", &dict),
        t("urn:m", "urn:p", "urn:n", &dict),
        // functional property → same values
        t("urn:f", RDF_TYPE, &format!("{OWL}FunctionalProperty"), &dict),
        t("urn:s", "urn:f", "urn:v1", &dict),
        t("urn:s", "urn:f", "urn:v2", &dict),
    ];
    // hasValue: x type Restr, Restr onProperty p / hasValue y → x p y
    input.push(t("urn:Restr", &format!("{OWL}onProperty"), "urn:p", &dict));
    input.push(t("urn:Restr", &format!("{OWL}hasValue"), "urn:y", &dict));
    input.push(t("urn:x", RDF_TYPE, "urn:Restr", &dict));

    let (outcome, report) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 64, None));
    assert!(!report.timed_out, "materialization must converge");
    assert!(!report.inconsistent, "input is consistent");
    assert!(
        report.inferred_triples >= 11,
        "expected >=11 inferences, got {}",
        report.inferred_triples
    );

    // transitive subclass closure
    assert_has(&outcome, &dict, "urn:A", &format!("{RDFS}subClassOf"), "urn:C");
    assert_has(&outcome, &dict, "urn:A", &format!("{RDFS}subClassOf"), "urn:D");
    assert_has(&outcome, &dict, "urn:B", &format!("{RDFS}subClassOf"), "urn:D");
    // domain/range typing
    assert_has(&outcome, &dict, "urn:x", RDF_TYPE, "urn:C");
    assert_has(&outcome, &dict, "urn:y", RDF_TYPE, "urn:D");
    // symmetric + transitive r
    assert_has(&outcome, &dict, "urn:b", "urn:r", "urn:a");
    assert_has(&outcome, &dict, "urn:a", "urn:r", "urn:c");
    assert_has(&outcome, &dict, "urn:c", "urn:r", "urn:a");
    // inverseOf
    assert_has(&outcome, &dict, "urn:n", "urn:q", "urn:m");
    // functional → sameAs
    assert_has(&outcome, &dict, "urn:v1", &format!("{OWL}sameAs"), "urn:v2");
    // hasValue: x p y re-derived
    assert_has(&outcome, &dict, "urn:x", "urn:p", "urn:y");
}

#[test]
fn inconsistency_detection_disjoint_and_alldifferent() {
    let dict = InMemoryDictionary::new();
    let mut input = vec![
        // cax-dw: disjoint classes share an instance
        t("urn:A", &format!("{OWL}disjointWith"), "urn:B", &dict),
        t("urn:x", RDF_TYPE, "urn:A", &dict),
        t("urn:x", RDF_TYPE, "urn:B", &dict),
        // eq-diff2: AllDifferent members are sameAs
        t("_:ad", RDF_TYPE, &format!("{OWL}AllDifferent"), &dict),
        t("_:ad", &format!("{OWL}members"), "_:l", &dict),
        t("urn:a", &format!("{OWL}sameAs"), "urn:b", &dict),
    ];
    input.extend(rdf_list(&dict, "_:l", &["urn:a", "urn:b"]));
    let (_, report) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 64, None));
    assert!(report.inconsistent, "disjoint + AllDifferent input must be inconsistent");
}

#[test]
fn property_chain_and_has_key() {
    let dict = InMemoryDictionary::new();
    let mut input = vec![
        t("urn:uncleOf", &format!("{OWL}propertyChainAxiom"), "_:u", &dict),
        t("urn:Person", &format!("{OWL}hasKey"), "_:k", &dict),
        t("urn:alice", RDF_TYPE, "urn:Person", &dict),
        t("urn:bob", RDF_TYPE, "urn:Person", &dict),
        t("urn:alice", "urn:ssn", "urn:v1", &dict),
        t("urn:bob", "urn:ssn", "urn:v1", &dict),
        t("urn:alice", "urn:siblingOf", "urn:bob", &dict),
        t("urn:bob", "urn:parentOf", "urn:carol", &dict),
    ];
    input.extend(rdf_list(&dict, "_:u", &["urn:siblingOf", "urn:parentOf"]));
    input.extend(rdf_list(&dict, "_:k", &["urn:ssn"]));
    let (outcome, report) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 64, None));
    assert!(!report.timed_out);
    assert_has(&outcome, &dict, "urn:alice", "urn:uncleOf", "urn:carol");
    assert_has(&outcome, &dict, "urn:alice", &format!("{OWL}sameAs"), "urn:bob");
}

#[test]
fn iteration_cap_bounds_closure() {
    let (dict, input, full_closure) = subclass_chain(40);
    // Two iterations propagate only a couple of hops down the chain: the
    // closure must be strictly partial (engine stays bounded).
    let (_, bounded) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 2, None));
    assert!(
        bounded.inferred_triples > 0 && bounded.inferred_triples < full_closure,
        "2-iteration cap should bound the closure below {full_closure}: got {}",
        bounded.inferred_triples
    );
    assert!(!bounded.timed_out, "iteration cap is not a wall-clock timeout");
    let (_, complete) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 64, None));
    assert_eq!(complete.inferred_triples, full_closure);
    assert!(!complete.timed_out);
}

#[test]
fn wall_clock_guardrail_times_out() {
    let (dict, input, full_closure) = subclass_chain(500);
    let started = Instant::now();
    let (_, report) = materialize(&dict, &input, task(InferenceMode::ForwardChaining, 4096, Some(1)));
    assert!(report.timed_out, "1ms budget must trip the wall-clock guardrail");
    assert!(
        report.inferred_triples < full_closure,
        "timed-out run must be partial: got {}",
        report.inferred_triples
    );
    assert!(
        started.elapsed().as_millis() < 10_000,
        "guardrail must return promptly"
    );
}

#[test]
fn large_closure_within_performance_budget() {
    // 40-node chain = 741 inferred subClassOf pairs; the forward-chaining
    // rule set must converge comfortably inside the wall-clock budget.
    let (dict, input, full_closure) = subclass_chain(40);
    let started = Instant::now();
    let (outcome, report) =
        materialize(&dict, &input, task(InferenceMode::ForwardChaining, 512, Some(5000)));
    assert!(!report.timed_out, "40-node chain must converge within budget");
    assert_eq!(
        report.inferred_triples, full_closure,
        "full transitive closure expected"
    );
    assert_eq!(outcome.len(), full_closure + input.len());
    let elapsed = started.elapsed().as_millis();
    assert!(
        elapsed < 5000,
        "40-node chain ({full_closure} inferences) took {elapsed}ms, over 5s budget"
    );
}

#[test]
fn mode_off_returns_input_unchanged() {
    let (dict, input, _) = subclass_chain(10);
    let (outcome, report) = materialize(&dict, &input, task(InferenceMode::Off, 64, None));
    assert_eq!(report.inferred_triples, 0);
    assert_eq!(outcome.len(), input.len());
}
