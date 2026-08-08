//! Manifest-driven W3C SHACL test suite runner (L6 P6-02).
//!
//! The official W3C SHACL 1.0 core test suite is vendored under
//! `tests/w3c-shacl/` (121 `sht:Validate` cases across `core/`). Each test
//! file is a Turtle document that acts as its own manifest: it declares an
//! `sht:Validate` entry whose `mf:action` points at the data and shapes graphs
//! (by default the file itself) and whose `mf:result` embeds the expected
//! `sh:ValidationReport`.
//!
//! This harness:
//!   - walks every `core/**/*.ttl` test file with our own Turtle parser,
//!   - runs the Ontolith `ShaclEngine` over the declared data/shapes graphs,
//!   - compares the produced report against the embedded expected report
//!     (conforms flag plus the set of result signatures),
//!   - locks outcomes in `tests/w3c-shacl_profile.tsv` so regressions fail CI
//!     while known gaps stay documented.
//!
//! Regenerate the profile after implementing a feature:
//!   ONTOLITH_SHACL_LEARN=1 cargo test -p ontolith-compliance --test shacl_suite

use ontolith_parser::application::RdfParser;
use ontolith_parser::domain::ParseRequest;
use ontolith_parser::infrastructure::BasicRdfParser;
use ontolith_rdf::domain::{Term, Triple};
use ontolith_reasoner::application::ShaclValidator;
use ontolith_reasoner::domain::{PropertyPath, ValidationResult, shacl};
use ontolith_reasoner::infrastructure::ShaclEngine;
use ontolith_storage::application::DictionaryCodec;
use ontolith_storage::infrastructure::InMemoryDictionary;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const MF_ACTION: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#action";
const MF_RESULT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result";
const SHT_VALIDATE: &str = "http://www.w3.org/ns/shacl-test#Validate";
const SHT_DATA_GRAPH: &str = "http://www.w3.org/ns/shacl-test#dataGraph";
const SHT_SHAPES_GRAPH: &str = "http://www.w3.org/ns/shacl-test#shapesGraph";
const SH_CONFORMS: &str = "http://www.w3.org/ns/shacl#conforms";
const SH_RESULT: &str = "http://www.w3.org/ns/shacl#result";
const SH_FOCUS_NODE: &str = "http://www.w3.org/ns/shacl#focusNode";
const SH_RESULT_PATH: &str = "http://www.w3.org/ns/shacl#resultPath";
const SH_RESULT_SEVERITY: &str = "http://www.w3.org/ns/shacl#resultSeverity";
const SH_SOURCE_CC: &str = "http://www.w3.org/ns/shacl#sourceConstraintComponent";
const SH_SOURCE_SHAPE: &str = "http://www.w3.org/ns/shacl#sourceShape";
const SH_VALUE: &str = "http://www.w3.org/ns/shacl#value";
const SH_VIOLATION: &str = "http://www.w3.org/ns/shacl#Violation";

type GraphMap = BTreeMap<String, Vec<(String, Term)>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FailReason {
    Parse(String),
    Unsupported(String),
    Semantic(String),
    Missing(String),
    Other(String),
}

impl FailReason {
    fn code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "parse-error",
            Self::Unsupported(_) => "unsupported",
            Self::Semantic(_) => "semantic",
            Self::Missing(_) => "missing",
            Self::Other(_) => "other",
        }
    }
}

struct TestOutcome {
    pass: bool,
    reason: Option<FailReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResultSig {
    focus: String,
    path: Option<String>,
    component: String,
    severity: String,
    value: Option<String>,
    source_shape: Option<String>,
}

#[derive(Debug, Clone)]
struct TestEntry {
    section: String,
    name: String,
    file: PathBuf,
    action: String,
    result: String,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn find_test_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("ttl")
                    && path.file_name().and_then(|n| n.to_str()) != Some("manifest.ttl")
                {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

fn file_uri(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", absolute.display())
}

fn load_graph(dict: &InMemoryDictionary, path: &Path) -> Result<Vec<Triple>, FailReason> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| FailReason::Missing(format!("read {}: {e}", path.display())))?;
    BasicRdfParser::new()
        .parse(
            &ParseRequest::turtle("test.ttl").with_base(file_uri(path)),
            &text,
            dict,
        )
        .map(|p| p.dataset.default_graph)
        .map_err(|e| FailReason::Parse(format!("{}: {e:?}", path.display())))
}

fn index_triples(triples: &[Triple], dict: &dyn DictionaryCodec) -> GraphMap {
    let mut map: GraphMap = BTreeMap::new();
    for t in triples {
        map.entry(subject_key(dict, t.subject))
            .or_default()
            .push((t.predicate.as_str().to_owned(), t.object.clone()));
    }
    map
}

fn subject_key(dict: &dyn DictionaryCodec, node: ontolith_core::domain::NodeId) -> String {
    dict.decode_node(node)
        .unwrap_or_else(|| format!("_:n{}", node.get()))
}

fn term_key(dict: &dyn DictionaryCodec, term: &Term) -> String {
    match term {
        Term::Iri(iri) => iri.as_str().to_owned(),
        Term::BlankNode(id) => subject_key(dict, *id),
        Term::Literal(v) => match v.language_tag() {
            Some(tag) => format!(
                "literal:{}|{}|{}",
                v.lexical_form(),
                v.xsd_datatype_iri(),
                tag.as_str()
            ),
            None => format!("literal:{}|{}", v.lexical_form(), v.xsd_datatype_iri()),
        },
    }
}

fn term_iri(term: &Term) -> Option<String> {
    match term {
        Term::Iri(iri) => Some(iri.as_str().to_owned()),
        _ => None,
    }
}

fn collect_entries(root: &Path, dict: &InMemoryDictionary) -> Vec<TestEntry> {
    let mut out = Vec::new();
    for path in find_test_files(root) {
        let Ok(triples) = load_graph(dict, &path) else {
            continue;
        };
        let map = index_triples(&triples, dict);
        for (subject, props) in &map {
            if !props
                .iter()
                .any(|(p, o)| p == RDF_TYPE && term_iri(o).as_deref() == Some(SHT_VALIDATE))
            {
                continue;
            }
            let name = subject
                .rsplit(['#', '/'])
                .next()
                .unwrap_or(subject)
                .to_owned();
            let section = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("core")
                .to_owned();
            let Some(action) = props
                .iter()
                .find(|(p, _)| p == MF_ACTION)
                .map(|(_, o)| term_key(dict, o))
            else {
                continue;
            };
            let Some(result) = props
                .iter()
                .find(|(p, _)| p == MF_RESULT)
                .map(|(_, o)| term_key(dict, o))
            else {
                continue;
            };
            out.push(TestEntry {
                section,
                name,
                file: path.clone(),
                action,
                result,
            });
        }
    }
    out.sort_by(|a, b| a.section.cmp(&b.section).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Resolve a `sht:dataGraph`/`sht:shapesGraph` object to the file it names.
fn resolve_graph(
    test_file: &Path,
    map: &GraphMap,
    action_key: &str,
    predicate: &str,
) -> Result<PathBuf, FailReason> {
    let self_uri = file_uri(test_file);
    let Some(term) = map
        .get(action_key)
        .and_then(|props| props.iter().find(|(p, _)| p == predicate))
        .map(|(_, o)| o)
    else {
        return Ok(test_file.to_path_buf());
    };
    let iri = term_iri(term).ok_or_else(|| {
        FailReason::Other(format!(
            "{} object is not an IRI in {}",
            predicate,
            test_file.display()
        ))
    })?;
    if iri == self_uri {
        return Ok(test_file.to_path_buf());
    }
    if let Some(rest) = iri.strip_prefix("file://") {
        return Ok(PathBuf::from(rest));
    }
    if iri.contains("://") {
        return Err(FailReason::Other(format!(
            "unresolvable graph reference {iri} in {}",
            test_file.display()
        )));
    }
    let parent = test_file.parent().unwrap_or_else(|| Path::new("."));
    Ok(parent.join(&iri))
}

// ---------------------------------------------------------------------------
// Expected report extraction
// ---------------------------------------------------------------------------

fn bool_literal(term: &Term) -> Option<bool> {
    match term {
        Term::Literal(ontolith_core::domain::LiteralValue::Boolean(b)) => Some(*b),
        _ => None,
    }
}

fn expected_conforms(map: &GraphMap, result_key: &str) -> Option<bool> {
    map.get(result_key)
        .and_then(|props| props.iter().find(|(p, _)| p == SH_CONFORMS))
        .and_then(|(_, o)| bool_literal(o))
}

fn expected_result_signature(
    dict: &dyn DictionaryCodec,
    map: &GraphMap,
    result_node: &str,
) -> Result<ResultSig, FailReason> {
    let props = map
        .get(result_node)
        .ok_or_else(|| FailReason::Other(format!("expected result node {result_node} missing")))?;
    let get = |p: &str| props.iter().find(|(q, _)| q == p).map(|(_, o)| o);
    let focus = get(SH_FOCUS_NODE)
        .map(|o| term_key(dict, o))
        .ok_or_else(|| {
            FailReason::Other(format!("expected result {result_node} lacks focusNode"))
        })?;
    let path = match get(SH_RESULT_PATH) {
        None => None,
        Some(o) => Some(expected_path(dict, map, o, &mut BTreeSet::new())?.canonical()),
    };
    let component = get(SH_SOURCE_CC).and_then(term_iri).unwrap_or_default();
    let severity = get(SH_RESULT_SEVERITY)
        .and_then(term_iri)
        .unwrap_or_else(|| SH_VIOLATION.to_owned());
    let value = get(SH_VALUE).map(|o| term_key(dict, o));
    let source_shape = get(SH_SOURCE_SHAPE).map(|o| term_key(dict, o));
    Ok(ResultSig {
        focus,
        path,
        component,
        severity,
        value,
        source_shape,
    })
}

/// Resolve an RDF collection starting at `start` into its members.
fn collect_expected_list(dict: &dyn DictionaryCodec, map: &GraphMap, start: &Term) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = start.clone();
    let mut seen = BTreeSet::new();
    loop {
        let key = term_key(dict, &cur);
        if !seen.insert(key.clone()) {
            break;
        }
        let Some(triples) = map.get(&key) else {
            break;
        };
        let first = triples
            .iter()
            .find(|(p, _)| p == RDF_FIRST)
            .map(|(_, o)| o.clone());
        let rest = triples
            .iter()
            .find(|(p, _)| p == RDF_REST)
            .map(|(_, o)| o.clone());
        match (first, rest) {
            (Some(f), Some(r)) => {
                out.push(f);
                cur = r;
            }
            (Some(f), None) => {
                out.push(f);
                break;
            }
            _ => break,
        }
    }
    out
}

/// Parse a SHACL property path from an expected `sh:resultPath` node (mirrors
/// the engine's `parse_path`: `rdf:first` sequences take precedence over the
/// `sh:*Path` predicates).
fn expected_path(
    dict: &dyn DictionaryCodec,
    map: &GraphMap,
    term: &Term,
    visiting: &mut BTreeSet<String>,
) -> Result<PropertyPath, FailReason> {
    match term {
        Term::Iri(iri) => Ok(PropertyPath::Predicate(iri.as_str().to_owned())),
        Term::BlankNode(_) => {
            let key = term_key(dict, term);
            if !visiting.insert(key.clone()) {
                return Err(FailReason::Unsupported(
                    "recursive expected result path".to_owned(),
                ));
            }
            let triples = map.get(&key).ok_or_else(|| {
                FailReason::Other(format!("expected result path node {key} missing"))
            })?;
            let result = if triples.iter().any(|(p, _)| p == RDF_FIRST) {
                let members = collect_expected_list(dict, map, term);
                let steps: Result<Vec<PropertyPath>, _> = members
                    .iter()
                    .map(|m| expected_path(dict, map, m, visiting))
                    .collect();
                steps.map(PropertyPath::Sequence)
            } else {
                let mut found: Option<Result<PropertyPath, FailReason>> = None;
                for (p, o) in triples {
                    match p.as_str() {
                        x if x == shacl("inversePath") => {
                            found = Some(
                                expected_path(dict, map, o, visiting)
                                    .map(Box::new)
                                    .map(PropertyPath::Inverse),
                            );
                            break;
                        }
                        x if x == shacl("alternativePath") => {
                            let branches: Result<Vec<PropertyPath>, _> =
                                collect_expected_list(dict, map, o)
                                    .iter()
                                    .map(|m| expected_path(dict, map, m, visiting))
                                    .collect();
                            found = Some(branches.map(PropertyPath::Alternative));
                            break;
                        }
                        x if x == shacl("zeroOrMorePath") => {
                            found = Some(
                                expected_path(dict, map, o, visiting)
                                    .map(Box::new)
                                    .map(PropertyPath::ZeroOrMore),
                            );
                            break;
                        }
                        x if x == shacl("oneOrMorePath") => {
                            found = Some(
                                expected_path(dict, map, o, visiting)
                                    .map(Box::new)
                                    .map(PropertyPath::OneOrMore),
                            );
                            break;
                        }
                        x if x == shacl("zeroOrOnePath") => {
                            found = Some(
                                expected_path(dict, map, o, visiting)
                                    .map(Box::new)
                                    .map(PropertyPath::ZeroOrOne),
                            );
                            break;
                        }
                        _ => {}
                    }
                }
                found.unwrap_or_else(|| {
                    Err(FailReason::Unsupported(
                        "unparseable expected result path".to_owned(),
                    ))
                })
            };
            visiting.remove(&key);
            result
        }
        Term::Literal(_) => Err(FailReason::Unsupported(
            "literal expected result path".to_owned(),
        )),
    }
}

fn expected_results(
    dict: &InMemoryDictionary,
    map: &GraphMap,
    result_key: &str,
) -> Result<Vec<ResultSig>, FailReason> {
    let Some(props) = map.get(result_key) else {
        return Err(FailReason::Other("expected report node missing".to_owned()));
    };
    let mut sigs = Vec::new();
    for (_, o) in props.iter().filter(|(p, _)| p == SH_RESULT) {
        let key = term_key(dict, o);
        sigs.push(expected_result_signature(dict, map, &key)?);
    }
    Ok(sigs)
}

fn actual_signature(r: &ValidationResult) -> ResultSig {
    ResultSig {
        focus: r.focus_node.clone(),
        path: r.path.clone(),
        component: r.component.clone(),
        severity: r.severity.clone().iri(),
        value: r.value.clone(),
        source_shape: r.source_shape.clone(),
    }
}

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

fn run_entry(entry: &TestEntry) -> TestOutcome {
    let dict = InMemoryDictionary::new();
    let fail = |reason: FailReason| TestOutcome {
        pass: false,
        reason: Some(reason),
    };
    let pass = TestOutcome {
        pass: true,
        reason: None,
    };

    let triples = match load_graph(&dict, &entry.file) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let map = index_triples(&triples, &dict);
    let data_file = match resolve_graph(&entry.file, &map, &entry.action, SHT_DATA_GRAPH) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    let shapes_file = match resolve_graph(&entry.file, &map, &entry.action, SHT_SHAPES_GRAPH) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };

    let data = match load_graph(&dict, &data_file) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let shapes = match load_graph(&dict, &shapes_file) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };

    let expected_conforms = match expected_conforms(&map, &entry.result) {
        Some(c) => c,
        None => {
            return fail(FailReason::Other(format!(
                "expected report {} lacks sh:conforms",
                entry.result
            )));
        }
    };
    let expected_results = match expected_results(&dict, &map, &entry.result) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };

    let report = match ShaclEngine::new().validate(&dict, &shapes, &data) {
        Ok(r) => r,
        Err(e) => return fail(FailReason::Other(format!("engine error: {e:?}"))),
    };
    let actual: BTreeSet<ResultSig> = report.results.iter().map(actual_signature).collect();
    let expected: BTreeSet<ResultSig> = expected_results.into_iter().collect();

    if report.conforms != expected_conforms {
        return fail(FailReason::Semantic(format!(
            "conforms expected {expected_conforms}, got {} ({} results expected, {} actual)",
            report.conforms,
            expected.len(),
            actual.len()
        )));
    }
    if actual != expected {
        let missing: Vec<&ResultSig> = expected.difference(&actual).collect();
        let extra: Vec<&ResultSig> = actual.difference(&expected).collect();
        let fmt = |v: &[&ResultSig]| {
            v.iter()
                .map(|s| {
                    format!(
                        "({} path={:?} cc={} sev={} val={:?} src={:?})",
                        s.focus, s.path, s.component, s.severity, s.value, s.source_shape
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        return fail(FailReason::Semantic(format!(
            "report mismatch: {} missing, {} extra (expected {}, got {})\n  missing: {}\n  extra:   {}",
            missing.len(),
            extra.len(),
            expected.len(),
            actual.len(),
            fmt(&missing),
            fmt(&extra)
        )));
    }
    pass
}

// ---------------------------------------------------------------------------
// Profile lock (mirrors tests/w3c11_suite.rs)
// ---------------------------------------------------------------------------

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .ok()
            .as_deref()
            .map(str::trim)
            .unwrap_or(""),
        "1" | "true" | "yes" | "on"
    )
}

fn load_profile(path: &Path) -> BTreeMap<(String, String), bool> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let mut parts = line.split('\t');
        let section = parts.next().unwrap_or("").to_owned();
        let name = parts.next().unwrap_or("").to_owned();
        let outcome = parts.next().unwrap_or("");
        if section.is_empty() || name.is_empty() {
            continue;
        }
        out.insert((section, name), outcome == "PASS");
    }
    out
}

fn write_profile(path: &Path, outcomes: &BTreeMap<(String, String), TestOutcome>) {
    let mut lines: Vec<String> = outcomes
        .iter()
        .map(|((section, name), outcome)| {
            let outcome_str = if outcome.pass { "PASS" } else { "FAIL" };
            let reason = outcome
                .reason
                .as_ref()
                .map(|r| r.code().to_owned())
                .unwrap_or_default();
            format!("{section}\t{name}\t{outcome_str}\t{reason}")
        })
        .collect();
    lines.sort();
    let mut out = String::from(
        "# W3C SHACL core suite profile (generated by tests/shacl_suite.rs).\n\
         # section<TAB>name<TAB>PASS|FAIL<TAB>reason-code\n",
    );
    out.push_str(&lines.join("\n"));
    out.push('\n');
    std::fs::write(path, out).expect("write shacl profile");
}

#[test]
fn shacl_w3c_core_suite() {
    let suite_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c-shacl/core");
    let profile_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c-shacl_profile.tsv");
    let learn = env_flag("ONTOLITH_SHACL_LEARN");

    let dict = InMemoryDictionary::new();
    let entries = collect_entries(&suite_root, &dict);
    assert!(
        !entries.is_empty(),
        "no W3C SHACL entries discovered under {}",
        suite_root.display()
    );

    let mut outcomes: BTreeMap<(String, String), TestOutcome> = BTreeMap::new();
    for entry in &entries {
        let outcome = run_entry(entry);
        println!(
            "[shacl] {} / {} :: {} {} {}",
            entry.section,
            entry.name,
            if outcome.pass { "PASS" } else { "FAIL" },
            outcome.reason.as_ref().map(|r| r.code()).unwrap_or(""),
            outcome
                .reason
                .as_ref()
                .map(|r| match r {
                    FailReason::Semantic(d)
                    | FailReason::Parse(d)
                    | FailReason::Unsupported(d)
                    | FailReason::Missing(d)
                    | FailReason::Other(d) => d.clone(),
                })
                .unwrap_or_default()
        );
        outcomes.insert((entry.section.clone(), entry.name.clone()), outcome);
    }

    if learn {
        write_profile(&profile_path, &outcomes);
        println!(
            "[shacl] wrote profile {} ({} entries)",
            profile_path.display(),
            outcomes.len()
        );
        return;
    }

    let profile = load_profile(&profile_path);
    assert!(
        !profile.is_empty(),
        "missing profile {}; run: ONTOLITH_SHACL_LEARN=1 cargo test -p ontolith-compliance --test shacl_suite",
        profile_path.display()
    );

    let mut drift = Vec::new();
    let mut missing = Vec::new();
    for (key, outcome) in &outcomes {
        let Some(expected_pass) = profile.get(key) else {
            missing.push(key.clone());
            continue;
        };
        if outcome.pass != *expected_pass {
            drift.push((
                key.clone(),
                *expected_pass,
                outcome.pass,
                outcome
                    .reason
                    .as_ref()
                    .map(|r| r.code().to_owned())
                    .unwrap_or_default(),
            ));
        }
    }

    let pass_count = outcomes.values().filter(|o| o.pass).count();
    let fail_count = outcomes.len() - pass_count;
    println!(
        "[shacl summary] total={} pass(must-pass)={} fail(known-gap)={} drift={} missing={}",
        outcomes.len(),
        pass_count,
        fail_count,
        drift.len(),
        missing.len()
    );

    for key in &missing {
        eprintln!("[shacl] missing profile entry: {}/{}", key.0, key.1);
    }
    for (key, expected, actual, reason) in &drift {
        eprintln!(
            "[shacl] drift {}/{}: expected {} got {} ({})",
            key.0,
            key.1,
            if *expected { "PASS" } else { "FAIL" },
            if *actual { "PASS" } else { "FAIL" },
            reason
        );
    }

    assert!(
        missing.is_empty(),
        "{} suite entries missing from profile (regenerate with ONTOLITH_SHACL_LEARN=1)",
        missing.len()
    );
    assert!(
        drift.is_empty(),
        "{} profile drifts: regress (or regenerate profile after implementing features)",
        drift.len()
    );
}
