//! Manifest-driven W3C SPARQL 1.1 test suite runner (L3 compliance, full-suite track).
//!
//! The official W3C SPARQL 1.1 test suite is vendored under `tests/w3c11/`
//! (query evaluation, update evaluation, and syntax manifests). This harness:
//!   - walks every feature manifest with our own Turtle parser,
//!   - executes each case through the Ontolith query pipeline,
//!   - compares against official expected results (SRX / SRJ / TSV / CSV /
//!     Turtle graphs / boolean),
//!   - locks outcomes in `tests/w3c11_profile.tsv` so regressions fail CI
//!     while known gaps stay documented.
//!
//! Regenerate the profile after implementing a feature:
//!   ONTOLITH_W3C11_LEARN=1 cargo test -p ontolith-compliance --test w3c11_suite

use ontolith_core::domain::{Iri, LanguageTag, LiteralValue, NodeId};
use ontolith_parser::application::RdfParser;
use ontolith_parser::domain::ParseRequest;
use ontolith_parser::infrastructure::term_lex::coerce_typed_literal;
use ontolith_parser::infrastructure::{BasicRdfParser, parse_ntriples, parse_turtle_doc};
use ontolith_query::domain::{BoundValue, QueryKind, QueryRequest, QueryResult};
use ontolith_query::infrastructure::{plan_query, update_pipeline};
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_storage::application::{
    DictionaryCodec, QuadRepository, StorageEngine, TripleRepository,
};
use ontolith_storage::infrastructure::{
    InMemoryDictionary, InMemoryQuadRepository, InMemoryStorageEngine, InMemoryTripleRepository,
};
use ontolith_transaction::domain::TxnId;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RS_RESULT_SET: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#ResultSet";
const RS_RESULT_VARIABLE: &str =
    "http://www.w3.org/2001/sw/DataAccess/tests/result-set#resultVariable";
const RS_SOLUTION: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#solution";
const RS_BINDING: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#binding";
const RS_VALUE: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#value";
const RS_VARIABLE: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#variable";

const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

type ManifestGraph = Vec<(NodeId, Vec<(Iri, Term)>)>;
type ResultRows = (Vec<String>, Vec<BTreeMap<String, NormTerm>>);
type ResultTable = (Vec<String>, Vec<BTreeMap<String, NormTerm>>, Option<bool>);

const QUERY_EVAL: &str =
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#QueryEvaluationTest";
const UPDATE_EVAL: &str =
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#UpdateEvaluationTest";
const POS_SYNTAX_TYPES: &[&str] = &[
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#PositiveSyntaxTest",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#PositiveSyntaxTest11",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#PositiveUpdateSyntaxTest",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#PositiveUpdateSyntaxTest11",
];
const NEG_SYNTAX_TYPES: &[&str] = &[
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#NegativeSyntaxTest",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#NegativeSyntaxTest11",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#NegativeUpdateSyntaxTest",
    "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#NegativeUpdateSyntaxTest11",
];

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
const XSD_FLOAT: &str = "http://www.w3.org/2001/XMLSchema#float";
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";

const EXEC_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestKind {
    QueryEvaluation,
    UpdateEvaluation,
    PositiveSyntax,
    NegativeSyntax,
}

#[derive(Debug, Clone)]
struct NamedGraphFile {
    name: String,
    file: PathBuf,
}

#[derive(Debug, Clone)]
enum Expected {
    Table(PathBuf),
    Graph(PathBuf),
    /// Update result store: optional default-graph post file plus expected
    /// named-graph contents.
    UpdateStore {
        default: Option<PathBuf>,
        named: Vec<NamedGraphFile>,
    },
    Boolean(bool),
    None,
}

#[derive(Debug, Clone)]
struct TestEntry {
    feature: String,
    name: String,
    kind: TestKind,
    request_file: Option<PathBuf>,
    data_files: Vec<PathBuf>,
    graph_data: Vec<NamedGraphFile>,
    expected: Expected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FailReason {
    DataFormat(String),
    ResultFormat(String),
    Unsupported(String),
    Semantic(String),
    ParseFailed,
    AcceptedInvalidSyntax,
    Timeout,
    Missing(String),
    Other(String),
}

impl FailReason {
    fn code(&self) -> &'static str {
        match self {
            Self::DataFormat(_) => "data-format",
            Self::ResultFormat(_) => "result-format",
            Self::Unsupported(_) => "unsupported",
            Self::Semantic(_) => "semantic",
            Self::ParseFailed => "parse-error",
            Self::AcceptedInvalidSyntax => "accepted-invalid",
            Self::Timeout => "timeout",
            Self::Missing(_) => "missing",
            Self::Other(_) => "other",
        }
    }
}

struct TestOutcome {
    pass: bool,
    reason: Option<FailReason>,
}

// ---------------------------------------------------------------------------
// Manifest walking
// ---------------------------------------------------------------------------

fn file_uri(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", absolute.display())
}

fn find_manifests(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("manifest.ttl") {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

fn parse_manifest(path: &Path, dict: &InMemoryDictionary) -> Result<ManifestGraph, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read manifest {}: {e}", path.display()))?;
    let parsed = BasicRdfParser::new()
        .parse(
            &ParseRequest::turtle("manifest.ttl").with_base(file_uri(path)),
            &text,
            dict,
        )
        .map_err(|e| format!("parse manifest {}: {e:?}", path.display()))?;
    let mut map: BTreeMap<NodeId, Vec<(Iri, Term)>> = BTreeMap::new();
    for triple in parsed.dataset.default_graph {
        map.entry(triple.subject)
            .or_default()
            .push((triple.predicate, triple.object));
    }
    Ok(map.into_iter().collect())
}

fn classify_kind(triples: &[(Iri, Term)]) -> Option<TestKind> {
    for (_, object) in triples {
        if let Term::Iri(t) = object {
            let iri = t.as_str();
            if iri == QUERY_EVAL {
                return Some(TestKind::QueryEvaluation);
            }
            if iri == UPDATE_EVAL {
                return Some(TestKind::UpdateEvaluation);
            }
            if POS_SYNTAX_TYPES.contains(&iri) {
                return Some(TestKind::PositiveSyntax);
            }
            if NEG_SYNTAX_TYPES.contains(&iri) {
                return Some(TestKind::NegativeSyntax);
            }
        }
    }
    None
}

fn term_iri(term: &Term) -> Option<String> {
    match term {
        Term::Iri(i) => Some(i.as_str().to_owned()),
        _ => None,
    }
}

fn term_literal_str(term: &Term) -> Option<String> {
    match term {
        Term::Literal(l) => Some(l.lexical_form()),
        _ => None,
    }
}

fn collect_list(subject: NodeId, map: &BTreeMap<NodeId, Vec<(Iri, Term)>>) -> Vec<Term> {
    let mut out = Vec::new();
    let mut current = Some(subject);
    while let Some(node) = current {
        let triples = match map.get(&node) {
            Some(t) => t,
            None => break,
        };
        let mut first = None;
        let mut rest = None;
        for (p, o) in triples {
            if p.as_str() == RDF_FIRST {
                first = Some(o.clone());
            } else if p.as_str() == RDF_REST {
                rest = Some(o.clone());
            }
        }
        if let Some(f) = first {
            out.push(f);
        }
        current = match rest {
            Some(Term::Iri(r)) if r.as_str() == RDF_NIL => None,
            Some(Term::BlankNode(id)) => Some(id),
            _ => None,
        };
    }
    out
}

fn collect_files(
    objects: &[Term],
    map: &BTreeMap<NodeId, Vec<(Iri, Term)>>,
    base_dir: &Path,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for object in objects {
        match object {
            Term::Iri(i) => out.push(relative_path(base_dir, i.as_str())),
            Term::BlankNode(id) => {
                if let Some(triples) = map.get(id) {
                    let mut first = None;
                    let mut rest = None;
                    for (p, o) in triples {
                        if p.as_str() == RDF_FIRST {
                            first = Some(o.clone());
                        } else if p.as_str() == RDF_REST {
                            rest = Some(o.clone());
                        }
                    }
                    let mut list = Vec::new();
                    if let Some(f) = first {
                        list.push(f);
                    }
                    if let Some(Term::BlankNode(next)) = rest {
                        list.extend(collect_list(next, map));
                    }
                    out.extend(collect_files(&list, map, base_dir));
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_named_graphs(
    objects: &[Term],
    map: &BTreeMap<NodeId, Vec<(Iri, Term)>>,
    base_dir: &Path,
) -> Vec<NamedGraphFile> {
    let mut out = Vec::new();
    for object in objects {
        let (file, label) = match object {
            // Direct `qt:graphData <file.ttl>` declarations (no label).
            Term::Iri(i) => (Some(relative_path(base_dir, i.as_str())), None),
            Term::BlankNode(id) => {
                let Some(triples) = map.get(id) else { continue };
                let mut file = None;
                let mut name = None;
                for (p, o) in triples {
                    if (p.as_str() == UT_GRAPH || p.as_str() == QT_GRAPH)
                        && let Some(iri) = term_iri(o)
                    {
                        file = Some(relative_path(base_dir, &iri));
                    } else if p.as_str() == RDFS_LABEL
                        && let Some(label) = term_literal_str(o)
                    {
                        name = Some(label);
                    }
                }
                (file, name)
            }
            _ => continue,
        };
        if let Some(file) = file {
            // Manifests without an rdfs:label (e.g.
            // aggregates/agg-empty-group-count-graph) name the graph after
            // the data file.
            let name = label.unwrap_or_else(|| {
                file.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("graph")
                    .to_owned()
            });
            out.push(NamedGraphFile { name, file });
        }
    }
    out
}

fn relative_path(base_dir: &Path, iri: &str) -> PathBuf {
    // Manifest IRIs resolve to file://... absolute paths; recover the
    // filesystem path from the last segment(s) that exist under base_dir.
    let stem = iri.rsplit('/').next().unwrap_or(iri);
    let direct = base_dir.join(stem);
    if direct.exists() {
        return direct;
    }
    // Fall back to treating the IRI tail as a relative path (nested dirs).
    let tail = iri.trim_start_matches("file://");
    let candidate = base_dir.join(tail);
    if candidate.exists() {
        return candidate;
    }
    direct
}

fn collect_entries(suite_root: &Path) -> Vec<TestEntry> {
    let mut out = Vec::new();
    for manifest in find_manifests(suite_root) {
        let feature = manifest
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_owned();
        let base_dir = manifest.parent().unwrap_or(suite_root).to_path_buf();
        let dict = InMemoryDictionary::new();
        let map = match parse_manifest(&manifest, &dict) {
            Ok(map) => map,
            Err(err) => {
                eprintln!("[w3c11] manifest walk error {feature}: {err}");
                continue;
            }
        };
        let map_ref: BTreeMap<NodeId, Vec<(Iri, Term)>> = map.into_iter().collect();
        let mut subjects: Vec<NodeId> = map_ref.keys().copied().collect();
        subjects.sort_by_key(|id| id.get());
        for subject in subjects {
            let triples = &map_ref[&subject];
            let Some(kind) = classify_kind(triples) else {
                continue;
            };
            out.push(build_entry(
                &feature, subject, &map_ref, &dict, kind, &base_dir,
            ));
        }
    }
    out
}

fn build_entry(
    feature: &str,
    subject: NodeId,
    map: &BTreeMap<NodeId, Vec<(Iri, Term)>>,
    dict: &InMemoryDictionary,
    kind: TestKind,
    base_dir: &Path,
) -> TestEntry {
    let triples = &map[&subject];
    let name = triples
        .iter()
        .find(|(p, _)| p.as_str() == MF_NAME)
        .and_then(|(_, o)| term_literal_str(o))
        .or_else(|| {
            triples
                .iter()
                .find(|(p, _)| p.as_str() == "http://www.w3.org/2000/01/rdf-schema#label")
                .and_then(|(_, o)| term_literal_str(o))
        })
        .unwrap_or_else(|| {
            dict.decode_node(subject)
                .and_then(|iri| iri.rsplit(['#', '/']).next().map(str::to_owned))
                .unwrap_or_else(|| format!("node{}", subject.get()))
        });

    let mut request_file = None;
    let mut data_objects = Vec::new();
    let mut graph_data_objects = Vec::new();
    let mut expected = Expected::None;

    for (p, o) in triples {
        if p.as_str() == MF_ACTION {
            match o {
                Term::Iri(i) => {
                    request_file = Some(relative_path(base_dir, i.as_str()));
                }
                Term::BlankNode(id) => {
                    if let Some(action_triples) = map.get(id) {
                        for (ap, ao) in action_triples {
                            match ap.as_str() {
                                QT_QUERY => {
                                    if let Some(iri) = term_iri(ao) {
                                        request_file = Some(relative_path(base_dir, &iri));
                                    }
                                }
                                UT_REQUEST => {
                                    if let Some(iri) = term_iri(ao) {
                                        request_file = Some(relative_path(base_dir, &iri));
                                    }
                                }
                                QT_DATA | UT_DATA => data_objects.push(ao.clone()),
                                QT_GRAPH_DATA | UT_GRAPH_DATA => {
                                    graph_data_objects.push(ao.clone())
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        } else if p.as_str() == MF_RESULT {
            match o {
                Term::Iri(i) => expected = Expected::Table(relative_path(base_dir, i.as_str())),
                Term::BlankNode(id) => {
                    if let Some(result_triples) = map.get(id) {
                        let mut boolean = None;
                        let mut post_default = None;
                        let mut post_named = Vec::new();
                        for (rp, ro) in result_triples {
                            if rp.as_str() == MF_BOOLEAN {
                                if let Term::Literal(l) = ro {
                                    boolean = Some(matches!(l, LiteralValue::Boolean(true)));
                                }
                            } else if (rp.as_str() == UT_DATA || rp.as_str() == QT_DATA)
                                && let Some(iri) = term_iri(ro)
                            {
                                post_default = Some(relative_path(base_dir, &iri));
                            } else if rp.as_str() == UT_GRAPH_DATA || rp.as_str() == QT_GRAPH_DATA {
                                post_named.extend(collect_named_graphs(
                                    std::slice::from_ref(ro),
                                    map,
                                    base_dir,
                                ));
                            }
                        }
                        if let Some(b) = boolean {
                            expected = Expected::Boolean(b);
                        } else if post_default.is_some() || !post_named.is_empty() {
                            expected = Expected::UpdateStore {
                                default: post_default,
                                named: post_named,
                            };
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let data_files = collect_files(&data_objects, map, base_dir);
    let graph_data = collect_named_graphs(&graph_data_objects, map, base_dir);

    // Distinguish table vs graph expected by extension.
    if let Expected::Table(path) = &expected {
        match path.extension().and_then(|e| e.to_str()) {
            Some("ttl") | Some("nt") | Some("nq") | Some("trig") | Some("rdf") | Some("owl")
            | Some("xml") => {
                expected = Expected::Graph(path.clone());
            }
            _ => {}
        }
    }

    TestEntry {
        feature: feature.to_owned(),
        name,
        kind,
        request_file,
        data_files,
        graph_data,
        expected,
    }
}

// Constants referenced by manifest walking.
const QT_QUERY: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#query";
const QT_DATA: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#data";
const QT_GRAPH_DATA: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#graphData";
const UT_REQUEST: &str = "http://www.w3.org/2009/sparql/tests/test-update#request";
const UT_DATA: &str = "http://www.w3.org/2009/sparql/tests/test-update#data";
const UT_GRAPH_DATA: &str = "http://www.w3.org/2009/sparql/tests/test-update#graphData";
const UT_GRAPH: &str = "http://www.w3.org/2009/sparql/tests/test-update#graph";
const QT_GRAPH: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#graph";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const MF_NAME: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#name";
const MF_ACTION: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#action";
const MF_RESULT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#result";
const MF_BOOLEAN: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#boolean";

// ---------------------------------------------------------------------------
// Term normalization for result comparison
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NormDt {
    Plain,
    Integer,
    Numeric(u64),
    Boolean,
    Lang,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NormTerm {
    Iri(String),
    Blank(String),
    Literal {
        lex: String,
        dt: NormDt,
        lang: Option<String>,
    },
}

fn norm_literal(lex: &str, datatype: Option<&str>, lang: Option<&str>) -> NormTerm {
    if let Some(l) = lang
        && !l.is_empty()
    {
        return NormTerm::Literal {
            lex: lex.to_owned(),
            dt: NormDt::Lang,
            // BCP 47 language tags compare case-insensitively.
            lang: Some(l.to_ascii_lowercase()),
        };
    }
    let dt = datatype.unwrap_or(XSD_STRING);
    match dt {
        XSD_BOOLEAN => {
            let b = matches!(lex.trim(), "true" | "1");
            NormTerm::Literal {
                lex: if b {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                },
                dt: NormDt::Boolean,
                lang: None,
            }
        }
        XSD_INTEGER
        | "http://www.w3.org/2001/XMLSchema#long"
        | "http://www.w3.org/2001/XMLSchema#int"
        | "http://www.w3.org/2001/XMLSchema#short"
        | "http://www.w3.org/2001/XMLSchema#byte"
        | "http://www.w3.org/2001/XMLSchema#nonNegativeInteger"
        | "http://www.w3.org/2001/XMLSchema#nonPositiveInteger"
        | "http://www.w3.org/2001/XMLSchema#positiveInteger"
        | "http://www.w3.org/2001/XMLSchema#negativeInteger"
        | "http://www.w3.org/2001/XMLSchema#unsignedLong"
        | "http://www.w3.org/2001/XMLSchema#unsignedInt"
        | "http://www.w3.org/2001/XMLSchema#unsignedShort"
        | "http://www.w3.org/2001/XMLSchema#unsignedByte" => match lex.parse::<i64>() {
            Ok(_) => NormTerm::Literal {
                lex: lex.to_owned(),
                dt: NormDt::Integer,
                lang: None,
            },
            Err(_) => NormTerm::Literal {
                lex: lex.to_owned(),
                dt: NormDt::Other(dt.to_owned()),
                lang: None,
            },
        },
        XSD_FLOAT => match lex.parse::<f32>() {
            Ok(v) => {
                let bits = (v as f64).to_bits();
                if v.is_nan() {
                    NormTerm::Literal {
                        lex: lex.to_owned(),
                        dt: NormDt::Other("numeric-nan".to_owned()),
                        lang: None,
                    }
                } else {
                    NormTerm::Literal {
                        lex: format!("n{bits}"),
                        dt: NormDt::Numeric(bits),
                        lang: None,
                    }
                }
            }
            Err(_) => NormTerm::Literal {
                lex: lex.to_owned(),
                dt: NormDt::Other(dt.to_owned()),
                lang: None,
            },
        },
        XSD_DECIMAL | XSD_DOUBLE => match lex.parse::<f64>() {
            Ok(v) => {
                let bits = v.to_bits();
                if v.is_nan() {
                    NormTerm::Literal {
                        lex: lex.to_owned(),
                        dt: NormDt::Other("numeric-nan".to_owned()),
                        lang: None,
                    }
                } else {
                    NormTerm::Literal {
                        lex: format!("n{bits}"),
                        dt: NormDt::Numeric(bits),
                        lang: None,
                    }
                }
            }
            Err(_) => NormTerm::Literal {
                lex: lex.to_owned(),
                dt: NormDt::Other(dt.to_owned()),
                lang: None,
            },
        },
        XSD_STRING => NormTerm::Literal {
            lex: lex.to_owned(),
            dt: NormDt::Plain,
            lang: None,
        },
        _ => NormTerm::Literal {
            lex: lex.to_owned(),
            dt: NormDt::Other(dt.to_owned()),
            lang: None,
        },
    }
}

fn norm_from_engine(value: &BoundValue, dict: Option<&InMemoryDictionary>) -> NormTerm {
    match value {
        BoundValue::Iri(i) => NormTerm::Iri(i.as_str().to_owned()),
        BoundValue::Literal(l) => match l {
            LiteralValue::String(s) => NormTerm::Literal {
                lex: s.clone(),
                dt: NormDt::Plain,
                lang: None,
            },
            LiteralValue::Integer(n) => NormTerm::Literal {
                lex: n.to_string(),
                dt: NormDt::Integer,
                lang: None,
            },
            LiteralValue::Decimal(d) => NormTerm::Literal {
                lex: format!("n{}", d.to_bits()),
                dt: NormDt::Numeric(d.to_bits()),
                lang: None,
            },
            LiteralValue::Double(d) => NormTerm::Literal {
                lex: format!("n{}", d.to_bits()),
                dt: NormDt::Numeric(d.to_bits()),
                lang: None,
            },
            LiteralValue::Float(f) => NormTerm::Literal {
                lex: format!("n{}", (*f as f64).to_bits()),
                dt: NormDt::Numeric((*f as f64).to_bits()),
                lang: None,
            },
            LiteralValue::Lang { value, lang } => NormTerm::Literal {
                lex: value.clone(),
                dt: NormDt::Lang,
                lang: Some(lang.as_str().to_owned()),
            },
            LiteralValue::Typed { value, datatype } => {
                // RDF 1.1: xsd:string typed literals equal simple literals;
                // other typed literals follow the same value normalization as
                // expected results (`norm_literal`), so non-canonical
                // lexemes such as `"0"^^xsd:boolean` compare by value.
                norm_literal(value, Some(datatype.as_str()), None)
            }
            LiteralValue::Boolean(b) => NormTerm::Literal {
                lex: b.to_string(),
                dt: NormDt::Boolean,
                lang: None,
            },
        },
        BoundValue::Node(id) | BoundValue::Blank(id) => match dict.and_then(|d| d.decode_node(*id))
        {
            Some(s) if s.starts_with("_:") => NormTerm::Blank(s),
            Some(s) => NormTerm::Iri(s),
            None => NormTerm::Blank(format!("n{}", id.get())),
        },
    }
}

fn norm_row(row: &BTreeMap<String, NormTerm>) -> Vec<(String, NormTerm)> {
    // Blank node labels in query results are arbitrary; map them to
    // positionally stable tokens so comparison is label-insensitive.
    let mut bnode_map: BTreeMap<String, String> = BTreeMap::new();
    let mut counter = 0usize;
    let mut v: Vec<(String, NormTerm)> = row
        .iter()
        .map(|(k, t)| {
            let t = match t {
                NormTerm::Blank(label) => {
                    let token = bnode_map.entry(label.clone()).or_insert_with(|| {
                        let tok = format!("b{counter}");
                        counter += 1;
                        tok
                    });
                    NormTerm::Blank(token.clone())
                }
                other => other.clone(),
            };
            (k.clone(), t)
        })
        .collect();
    v.sort();
    v
}

fn row_set(rows: &[BTreeMap<String, NormTerm>]) -> BTreeSet<Vec<(String, NormTerm)>> {
    rows.iter().map(norm_row).collect()
}

// ---------------------------------------------------------------------------
// Expected-result readers
// ---------------------------------------------------------------------------

fn srx_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn parse_srx(text: &str) -> Result<ResultTable, String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut vars = Vec::new();
    let mut rows = Vec::new();
    let mut boolean: Option<bool> = None;
    let mut row: Option<BTreeMap<String, NormTerm>> = None;
    let mut current_var: Option<String> = None;
    let mut term_kind: Option<&'static str> = None;
    let mut literal_dt: Option<String> = None;
    let mut literal_lang: Option<String> = None;
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Err(e) => return Err(format!("srx xml error: {e}")),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"variable" => {
                    if let Some(name) = srx_attr(&e, b"name") {
                        vars.push(name);
                    }
                }
                b"result" => {
                    row = Some(BTreeMap::new());
                }
                b"binding" => {
                    current_var = srx_attr(&e, b"name");
                }
                b"uri" => term_kind = Some("uri"),
                b"bnode" => term_kind = Some("bnode"),
                b"literal" => {
                    term_kind = Some("literal");
                    literal_dt = srx_attr(&e, b"datatype");
                    literal_lang = srx_attr(&e, b"xml:lang").or_else(|| srx_attr(&e, b"lang"));
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"variable"
                    && let Some(name) = srx_attr(&e, b"name")
                {
                    vars.push(name);
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(decoded) = t.unescape() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"uri" | b"bnode" | b"literal" => {
                    let term = match term_kind {
                        Some("uri") => NormTerm::Iri(text_buf.clone()),
                        Some("bnode") => NormTerm::Blank(text_buf.clone()),
                        Some("literal") => {
                            norm_literal(&text_buf, literal_dt.as_deref(), literal_lang.as_deref())
                        }
                        _ => NormTerm::Literal {
                            lex: text_buf.clone(),
                            dt: NormDt::Plain,
                            lang: None,
                        },
                    };
                    if let (Some(row), Some(var)) = (&mut row, &current_var) {
                        row.insert(var.clone(), term);
                    }
                    text_buf.clear();
                    term_kind = None;
                    literal_dt = None;
                    literal_lang = None;
                }
                b"binding" => current_var = None,
                b"result" => {
                    if let Some(r) = row.take() {
                        rows.push(r);
                    }
                }
                b"boolean" => {
                    boolean = Some(text_buf.trim() == "true");
                    text_buf.clear();
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok((vars, rows, boolean))
}

fn parse_tsv_cell(cell: &str) -> Result<NormTerm, String> {
    let cell = cell.trim();
    if let Some(inner) = cell.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return Ok(NormTerm::Iri(inner.to_owned()));
    }
    if let Some(label) = cell.strip_prefix("_:") {
        return Ok(NormTerm::Blank(format!("_:{label}")));
    }
    if let Some(rest) = cell.strip_prefix('"') {
        let close = rest
            .find('"')
            .ok_or_else(|| format!("unterminated tsv literal: {cell}"))?;
        let lex = rest[..close].replace("\\\"", "\"").replace("\\\\", "\\");
        let suffix = &rest[close + 1..];
        let (dt, lang) = if let Some(l) = suffix.strip_prefix('@') {
            (None, Some(l.to_owned()))
        } else if let Some(d) = suffix.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
            (Some(d.to_owned()), None)
        } else {
            (None, None)
        };
        return Ok(norm_literal(&lex, dt.as_deref(), lang.as_deref()));
    }
    // Unquoted cell: numeric or boolean lexical form.
    if cell.parse::<i64>().is_ok() {
        return Ok(norm_literal(cell, Some(XSD_INTEGER), None));
    }
    if cell == "true" || cell == "false" {
        return Ok(norm_literal(cell, Some(XSD_BOOLEAN), None));
    }
    if cell.parse::<f64>().is_ok() {
        return Ok(norm_literal(cell, Some(XSD_DOUBLE), None));
    }
    Ok(norm_literal(cell, None, None))
}

fn parse_tsv(text: &str) -> Result<ResultRows, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().ok_or("empty tsv")?;
    let vars: Vec<String> = header
        .split('\t')
        .map(|v| v.trim().trim_start_matches('?').to_owned())
        .collect();
    let mut rows = Vec::new();
    for line in lines {
        let cells: Vec<&str> = line.split('\t').collect();
        if cells.len() != vars.len() {
            return Err(format!(
                "tsv row arity mismatch: expected {}, got {}",
                vars.len(),
                cells.len()
            ));
        }
        let mut row = BTreeMap::new();
        for (v, c) in vars.iter().zip(cells) {
            if c.trim().is_empty() {
                continue;
            }
            row.insert(v.clone(), parse_tsv_cell(c)?);
        }
        rows.push(row);
    }
    Ok((vars, rows))
}

fn parse_csv(text: &str) -> Result<ResultRows, String> {
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == '"' {
                if i + 1 < chars.len() && chars[i + 1] == '"' {
                    field.push('"');
                    i += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    current.push(std::mem::take(&mut field));
                }
                '\n' => {
                    current.push(std::mem::take(&mut field));
                    table.push(std::mem::take(&mut current));
                }
                '\r' => {}
                _ => field.push(c),
            }
        }
        i += 1;
    }
    if !field.is_empty() || !current.is_empty() {
        current.push(field);
        table.push(current);
    }
    let mut rows_iter = table
        .into_iter()
        .filter(|r| !r.iter().all(|c| c.trim().is_empty()));
    let header = rows_iter.next().ok_or("empty csv")?;
    let vars: Vec<String> = header
        .iter()
        .map(|v| v.trim().trim_start_matches('?').to_owned())
        .collect();
    let mut rows = Vec::new();
    for record in rows_iter {
        let mut row = BTreeMap::new();
        for (idx, cell) in record.iter().enumerate() {
            let cell = cell.trim();
            if cell.is_empty() || idx >= vars.len() {
                continue;
            }
            row.insert(vars[idx].clone(), parse_tsv_cell(cell)?);
        }
        rows.push(row);
    }
    Ok((vars, rows))
}

fn parse_srj(text: &str) -> Result<ResultTable, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let vars: Vec<String> = value["head"]["vars"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mut rows = Vec::new();
    if let Some(bindings) = value["results"]["bindings"].as_array() {
        for binding in bindings {
            let mut row = BTreeMap::new();
            if let Some(obj) = binding.as_object() {
                for (var, val) in obj {
                    let term_type = val["type"].as_str().unwrap_or("");
                    let term_value = val["value"].as_str().unwrap_or("");
                    let term = match term_type {
                        "uri" => NormTerm::Iri(term_value.to_owned()),
                        "bnode" => NormTerm::Blank(term_value.to_owned()),
                        "literal" => norm_literal(
                            term_value,
                            val["datatype"].as_str(),
                            val["xml:lang"].as_str().or_else(|| val["lang"].as_str()),
                        ),
                        _ => continue,
                    };
                    row.insert(var.clone(), term);
                }
            }
            rows.push(row);
        }
    }
    let boolean = value["boolean"].as_bool();
    Ok((vars, rows, boolean))
}

// ---------------------------------------------------------------------------
// Graph helpers
// ---------------------------------------------------------------------------

fn node_label(id: NodeId, dict: &InMemoryDictionary) -> String {
    dict.decode_node(id)
        .unwrap_or_else(|| format!("_:n{}", id.get()))
}

fn term_label(term: &Term, dict: &InMemoryDictionary) -> String {
    match term {
        Term::Iri(i) => format!("<{}>", i.as_str()),
        Term::BlankNode(id) => node_label(*id, dict),
        Term::Literal(l) => literal_canon(l),
    }
}

fn literal_canon(l: &LiteralValue) -> String {
    match l {
        LiteralValue::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        LiteralValue::Lang { value, lang } => {
            format!("\"{}\"@{}", value.replace('"', "\\\""), lang.as_str())
        }
        LiteralValue::Typed { value, datatype } => format!(
            "\"{}\"^^<{}>",
            value.replace('"', "\\\""),
            datatype.as_str()
        ),
        LiteralValue::Integer(n) => format!("num:i:{n}"),
        LiteralValue::Decimal(d) => format!("num:f:{}", d.to_bits()),
        LiteralValue::Float(f) => format!("num:f:{}", (*f as f64).to_bits()),
        LiteralValue::Double(d) => format!("num:f:{}", d.to_bits()),
        LiteralValue::Boolean(b) => format!("bool:{b}"),
    }
}

fn triple_canon(triple: &Triple, dict: &InMemoryDictionary) -> String {
    format!(
        "{} {} {}",
        node_label(triple.subject, dict),
        triple.predicate.as_str(),
        term_label(&triple.object, dict)
    )
}

fn graph_set(triples: &[Triple], dict: &InMemoryDictionary) -> BTreeSet<String> {
    // Blank node labels in graphs are arbitrary (data labels, query-minted
    // ids); map them to positionally stable tokens so graph comparison is
    // label-insensitive, mirroring `norm_row` for result tables.
    let raw: Vec<String> = triples.iter().map(|t| triple_canon(t, dict)).collect();
    let mut labels: Vec<&str> = raw
        .iter()
        .flat_map(|line| line.split(' '))
        .filter(|part| part.starts_with("_:"))
        .collect();
    labels.sort_unstable();
    labels.dedup();
    let tokens: BTreeMap<&str, String> = labels
        .into_iter()
        .enumerate()
        .map(|(i, label)| (label, format!("b{i}")))
        .collect();
    raw.iter()
        .map(|line| {
            line.split(' ')
                .map(|part| tokens.get(part).cloned().unwrap_or_else(|| part.to_owned()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Parse a W3C `rs:ResultSet` Turtle table (used by some aggregate tests
/// whose expected result is serialized as RDF rather than SRX/SRJ). The
/// files use relative IRIs (`<singleton.ttl>`) for graph-name bindings, so
/// the file's parent directory is used as the base IRI and relative values
/// are normalized back to their file name to match the harness's graph names.
fn parse_rdf_result_set(path: &Path) -> Result<ResultRows, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parent = path
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let base = format!(
        "file://{}/",
        parent.display().to_string().replace('\\', "/")
    );
    let dict = InMemoryDictionary::new();
    let parsed = BasicRdfParser::new()
        .parse(
            &ParseRequest::turtle("inline").with_base(base.clone()),
            &text,
            &dict,
        )
        .map_err(|e| format!("parse {}: {e:?}", path.display()))?;
    let triples = parsed.dataset.default_graph;
    let mut map: BTreeMap<NodeId, Vec<(Iri, Term)>> = BTreeMap::new();
    for t in &triples {
        map.entry(t.subject)
            .or_default()
            .push((t.predicate.clone(), t.object.clone()));
    }
    let mut root = None;
    for (subject, props) in &map {
        if props
            .iter()
            .any(|(p, o)| p.as_str() == RDF_TYPE && term_iri(o).as_deref() == Some(RS_RESULT_SET))
        {
            root = Some(*subject);
            break;
        }
    }
    let root = root.ok_or_else(|| format!("{} is not an rs:ResultSet", path.display()))?;
    let props = map
        .get(&root)
        .ok_or_else(|| "rs:ResultSet node has no properties".to_owned())?;
    let mut vars = Vec::new();
    let mut solutions = Vec::new();
    for (p, o) in props {
        if p.as_str() == RS_RESULT_VARIABLE {
            if let Some(v) = term_literal_str(o) {
                vars.push(v);
            }
        } else if p.as_str() == RS_SOLUTION
            && let Term::BlankNode(id) = o
        {
            solutions.push(*id);
        }
    }
    let mut rows = Vec::new();
    for sol in solutions {
        let mut row = BTreeMap::new();
        let Some(sol_props) = map.get(&sol) else {
            continue;
        };
        for (p, o) in sol_props {
            if p.as_str() != RS_BINDING {
                continue;
            }
            let Term::BlankNode(binding) = o else {
                continue;
            };
            let Some(bind_props) = map.get(binding) else {
                continue;
            };
            let mut var = None;
            let mut value = None;
            for (bp, bo) in bind_props {
                if bp.as_str() == RS_VARIABLE {
                    var = term_literal_str(bo);
                } else if bp.as_str() == RS_VALUE {
                    value = Some(bo.clone());
                }
            }
            if let (Some(var), Some(value)) = (var, value) {
                row.insert(var, term_to_norm(&value, &dict, &base));
            }
        }
        rows.push(row);
    }
    Ok((vars, rows))
}

fn term_to_norm(term: &Term, dict: &InMemoryDictionary, base: &str) -> NormTerm {
    match term {
        Term::Iri(i) => {
            let s = i.as_str();
            let s = s.strip_prefix(base).unwrap_or(s);
            NormTerm::Iri(s.to_owned())
        }
        Term::Literal(l) => match l {
            LiteralValue::Lang { value, lang } => norm_literal(value, None, Some(lang.as_str())),
            LiteralValue::Typed { value, datatype } => {
                norm_literal(value, Some(datatype.as_str()), None)
            }
            other => norm_literal(
                &other.lexical_form(),
                Some(other.xsd_datatype_iri().as_str()),
                None,
            ),
        },
        Term::BlankNode(id) => match dict.decode_node(*id) {
            Some(label) => NormTerm::Blank(label),
            None => NormTerm::Blank(format!("_:b{}", id.get())),
        },
    }
}

/// Minimal RDF/XML reader covering the constructs used by the vendored W3C
/// data files (`rdf:Description` + prefixed property elements, `rdf:about`,
/// `rdf:resource`, `rdf:nodeID`, `rdf:datatype`, `xml:lang`, text content).
/// Relative IRIs (including `rdf:resource=""`) resolve against `base`.
fn parse_rdf_xml(text: &str, dict: &InMemoryDictionary, base: &str) -> Result<Vec<Triple>, String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut ns: BTreeMap<String, String> = BTreeMap::new();
    ns.insert(
        "rdf".to_owned(),
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#".to_owned(),
    );
    let mut triples = Vec::new();
    let mut subject: Option<NodeId> = None;
    let mut pending: Option<(Iri, Option<String>, Option<String>)> = None; // (pred, datatype, lang)
    let mut text_buf = String::new();
    let mut in_root = false;

    let attr = |e: &quick_xml::events::BytesStart<'_>, key: &str| -> Option<String> {
        e.attributes()
            .filter_map(|a| a.ok())
            .find(|a| a.key.as_ref() == key.as_bytes())
            .map(|a| String::from_utf8_lossy(&a.value).into_owned())
    };
    let emit = |triples: &mut Vec<Triple>, subject: NodeId, pred: &Iri, object: &Term| {
        triples.push(Triple {
            subject,
            predicate: pred.clone(),
            object: object.clone(),
        });
    };
    loop {
        match reader.read_event() {
            Err(e) => return Err(format!("rdf/xml error: {e}")),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "rdf:RDF" {
                    in_root = true;
                    for a in e.attributes().filter_map(|a| a.ok()) {
                        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                        if let Some(prefix) = k.strip_prefix("xmlns:") {
                            let v = String::from_utf8_lossy(&a.value).into_owned();
                            ns.insert(prefix.to_owned(), v);
                        }
                    }
                    continue;
                }
                if !in_root {
                    continue;
                }
                if name == "rdf:Description" {
                    let about = attr(&e, "rdf:about");
                    let node_id = attr(&e, "rdf:nodeID");
                    let subject_term = if let Some(a) = about {
                        dict.encode_node(&resolve_iri(&a, base))
                    } else if let Some(n) = node_id {
                        dict.encode_node(&format!("_:{n}"))
                    } else {
                        dict.encode_node(&format!("_:rdfxml{}", dict.len()))
                    };
                    subject = Some(subject_term);
                    continue;
                }
                if subject.is_none() {
                    continue;
                }
                if let Some(pred) = resolve_prefixed_name(&name, &ns) {
                    let resource = attr(&e, "rdf:resource");
                    let node_id = attr(&e, "rdf:nodeID");
                    if let Some(r) = resource {
                        emit(
                            &mut triples,
                            subject.unwrap(),
                            &pred,
                            &Term::Iri(Iri::new(resolve_iri(&r, base))),
                        );
                    } else if let Some(n) = node_id {
                        emit(
                            &mut triples,
                            subject.unwrap(),
                            &pred,
                            &Term::BlankNode(dict.encode_node(&format!("_:{n}"))),
                        );
                    } else {
                        let datatype = attr(&e, "rdf:datatype");
                        let lang = attr(&e, "xml:lang");
                        pending = Some((pred, datatype, lang));
                        text_buf.clear();
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if subject.is_none() || name == "rdf:Description" {
                    continue;
                }
                if let Some(pred) = resolve_prefixed_name(&name, &ns) {
                    let resource = attr(&e, "rdf:resource");
                    let node_id = attr(&e, "rdf:nodeID");
                    let object = if let Some(r) = resource {
                        Term::Iri(Iri::new(resolve_iri(&r, base)))
                    } else if let Some(n) = node_id {
                        Term::BlankNode(dict.encode_node(&format!("_:{n}")))
                    } else {
                        Term::Literal(LiteralValue::String(String::new()))
                    };
                    emit(&mut triples, subject.unwrap(), &pred, &object);
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(decoded) = t.unescape() {
                    text_buf.push_str(&decoded);
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "rdf:Description" {
                    subject = None;
                } else if let (Some(s), Some((pred, datatype, lang))) = (subject, pending.take()) {
                    let lex = text_buf.trim().to_owned();
                    let object = match (datatype, lang) {
                        (Some(dt), _) => {
                            // Compact forms for known XSD datatypes, matching
                            // the Turtle/N-Triples parser (`coerce_typed_literal`).
                            Term::Literal(coerce_typed_literal(lex, &dt))
                        }
                        (None, Some(l)) => Term::Literal(LiteralValue::Lang {
                            value: lex,
                            lang: LanguageTag::parse(&l)
                                .map_err(|e| format!("bad xml:lang {l}: {e}"))?,
                        }),
                        (None, None) => Term::Literal(LiteralValue::String(lex)),
                    };
                    emit(&mut triples, s, &pred, &object);
                }
            }
            _ => {}
        }
    }
    Ok(triples)
}

/// Resolve a prefixed XML name (`ex:p`) against the collected namespaces.
fn resolve_prefixed_name(name: &str, ns: &BTreeMap<String, String>) -> Option<Iri> {
    if let Some((prefix, local)) = name.split_once(':') {
        let base = ns.get(prefix)?;
        Some(Iri::new(format!("{base}{local}")))
    } else {
        // Unprefixed element names are not valid property IRIs here.
        None
    }
}

/// RFC 3986-lite IRI resolution good enough for the vendored suite: empty
/// references resolve to the base, absolute references pass through, and
/// relative/fragment references join onto the base's last path segment.
fn resolve_iri(reference: &str, base: &str) -> String {
    if reference.is_empty() {
        return base.to_owned();
    }
    if reference.contains(':') {
        return reference.to_owned();
    }
    if let Some(fragment) = reference.strip_prefix('#') {
        let cut = base.rfind(['#', '/']).map(|i| i + 1).unwrap_or(base.len());
        return format!("{}{fragment}", &base[..cut]);
    }
    match base.rfind('/') {
        Some(i) => format!("{}/{}", &base[..i + 1], reference),
        None => reference.to_owned(),
    }
}

fn parse_graph_file(path: &Path) -> Result<(Vec<Triple>, InMemoryDictionary), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let dict = InMemoryDictionary::new();
    let triples = match path.extension().and_then(|e| e.to_str()) {
        Some("nt") => {
            parse_ntriples(&text, &dict)
                .map_err(|e| format!("parse {}: {e:?}", path.display()))?
                .dataset
                .default_graph
        }
        Some("rdf") => parse_rdf_xml(&text, &dict, &file_uri(path))
            .map_err(|e| format!("parse {}: {e}", path.display()))?,
        _ => {
            parse_turtle_doc(&text, &dict)
                .map_err(|e| format!("parse {}: {e:?}", path.display()))?
                .dataset
                .default_graph
        }
    };
    Ok((triples, dict))
}

// ---------------------------------------------------------------------------
// Dataset loading + execution
// ---------------------------------------------------------------------------

struct Loaded {
    engine: Arc<InMemoryStorageEngine>,
    repo: Arc<dyn TripleRepository>,
    dict: Arc<InMemoryDictionary>,
    quads: Arc<InMemoryQuadRepository>,
}

fn load_data(
    files: &[PathBuf],
    graph_data: &[NamedGraphFile],
    expose_data_graphs: bool,
) -> Result<Loaded, String> {
    let engine = Arc::new(InMemoryStorageEngine::new());
    let dict = Arc::new(InMemoryDictionary::new());
    let repo: Arc<dyn TripleRepository> =
        Arc::new(InMemoryTripleRepository::new(Arc::clone(&engine)));
    let quads = Arc::new(InMemoryQuadRepository::new(Arc::clone(&engine)));
    let txn = TxnId::new(1);
    let mut inserted = 0usize;
    for file in files {
        let text =
            std::fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?;
        // Query evaluation follows the vendored suite's bare-file-name
        // convention: the data file's base is its file name, so `<>` in
        // exists-graph-variable.ttl resolves to the same IRI the graphData
        // entry registers (the graph name doubles as the document IRI).
        let base = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_owned();
        let triples = match file.extension().and_then(|e| e.to_str()) {
            Some("nt") => {
                parse_ntriples(&text, dict.as_ref())
                    .map_err(|e| format!("parse {}: {e:?}", file.display()))?
                    .dataset
                    .default_graph
            }
            Some("rdf") => parse_rdf_xml(&text, dict.as_ref(), &base)
                .map_err(|e| format!("parse {}: {e}", file.display()))?,
            _ if expose_data_graphs => {
                BasicRdfParser::new()
                    .parse(
                        &ParseRequest::turtle("data.ttl").with_base(base.clone()),
                        &text,
                        dict.as_ref(),
                    )
                    .map_err(|e| format!("parse {}: {e:?}", file.display()))?
                    .dataset
                    .default_graph
            }
            _ => {
                parse_turtle_doc(&text, dict.as_ref())
                    .map_err(|e| format!("parse {}: {e:?}", file.display()))?
                    .dataset
                    .default_graph
            }
        };
        for triple in triples {
            repo.insert(txn, triple)
                .map_err(|e| format!("insert: {e:?}"))?;
            inserted += 1;
        }
    }
    for ng in graph_data {
        let text = std::fs::read_to_string(&ng.file)
            .map_err(|e| format!("read {}: {e}", ng.file.display()))?;
        // Data resolves relative IRIs against the graph name (the graph name
        // doubles as the document IRI in the W3C data files).
        let triples = match ng.file.extension().and_then(|e| e.to_str()) {
            Some("rdf") => parse_rdf_xml(&text, dict.as_ref(), &ng.name)
                .map_err(|e| format!("parse {}: {e}", ng.file.display()))?,
            _ => {
                BasicRdfParser::new()
                    .parse(
                        &ParseRequest::turtle("graph.ttl").with_base(ng.name.clone()),
                        &text,
                        dict.as_ref(),
                    )
                    .map_err(|e| format!("parse {}: {e:?}", ng.file.display()))?
                    .dataset
                    .default_graph
            }
        };
        let graph_name = Iri::new(ng.name.clone());
        for triple in triples {
            quads
                .insert(txn, Quad::in_named_graph(triple, graph_name.clone()))
                .map_err(|e| format!("insert quad: {e:?}"))?;
            inserted += 1;
        }
    }
    if inserted > 0 {
        engine
            .commit_transaction(txn)
            .map_err(|e| format!("commit: {e:?}"))?;
    }
    Ok(Loaded {
        engine,
        repo,
        dict,
        quads,
    })
}

fn execute_query(
    loaded: &Loaded,
    text: &str,
) -> Result<ontolith_query::domain::QueryResult, String> {
    let pipeline = update_pipeline(
        loaded.repo.clone(),
        loaded.engine.clone(),
        Some(loaded.dict.clone()),
    );
    let request = QueryRequest::new(text).with_timeout(EXEC_TIMEOUT_MS);
    let result =
        catch_unwind(AssertUnwindSafe(|| pipeline.execute(&request))).map_err(panic_payload)?;
    result.map_err(|e| e.message().to_owned())
}

fn panic_payload(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

fn parse_only(text: &str) -> Result<(), String> {
    let result = catch_unwind(AssertUnwindSafe(|| plan_query(&QueryRequest::new(text))))
        .map_err(panic_payload)?;
    result.map(|_| ()).map_err(|e| e.message().to_owned())
}

// ---------------------------------------------------------------------------
// Comparisons
// ---------------------------------------------------------------------------

fn compare_boolean(actual: &QueryResult, expected: bool) -> Result<(), String> {
    if actual.kind != QueryKind::Ask {
        return Err(format!("expected ASK, got {:?}", actual.kind));
    }
    if actual.boolean != Some(expected) {
        return Err(format!("expected ASK={expected}, got {:?}", actual.boolean));
    }
    Ok(())
}

fn compare_table(
    actual: &QueryResult,
    expected_vars: &[String],
    expected_rows: &[BTreeMap<String, NormTerm>],
    loaded: &Loaded,
) -> Result<(), String> {
    if actual.kind != QueryKind::Select {
        return Err(format!("expected SELECT, got {:?}", actual.kind));
    }
    let actual_rows: BTreeSet<Vec<(String, NormTerm)>> = actual
        .solutions
        .iter()
        .map(|s| {
            let row: BTreeMap<String, NormTerm> = s
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), norm_from_engine(v, Some(&loaded.dict))))
                .collect();
            norm_row(&row)
        })
        .collect();
    let expected_set = row_set(expected_rows);
    if actual_rows != expected_set {
        let expected_vars_set: BTreeSet<&String> = expected_vars.iter().collect();
        let actual_vars_set: BTreeSet<&String> = actual.variables.iter().collect();
        return Err(format!(
            "row set mismatch: expected {} rows / vars {:?}, got {} rows / vars {:?}",
            expected_rows.len(),
            expected_vars_set,
            actual.solutions.len(),
            actual_vars_set
        ));
    }
    Ok(())
}

fn compare_graph(
    actual: &QueryResult,
    expected_path: &Path,
    loaded: &Loaded,
) -> Result<(), String> {
    if actual.kind != QueryKind::Construct && actual.kind != QueryKind::Describe {
        return Err(format!(
            "expected CONSTRUCT/DESCRIBE, got {:?}",
            actual.kind
        ));
    }
    let (expected_triples, expected_dict) = parse_graph_file(expected_path)?;
    let expected_set = graph_set(&expected_triples, &expected_dict);
    let actual_set = graph_set(&actual.construct_triples, &loaded.dict);
    if actual_set != expected_set {
        return Err(format!(
            "graph mismatch: expected {} triples, got {}",
            expected_set.len(),
            actual_set.len()
        ));
    }
    Ok(())
}

fn compare_update_graph(loaded: &Loaded, expected_path: &Path) -> Result<(), String> {
    let actual_triples = loaded.repo.all_in_txn(None);
    let actual_set = graph_set(&actual_triples, &loaded.dict);
    let (expected_triples, expected_dict) = parse_graph_file(expected_path)?;
    let expected_set = graph_set(&expected_triples, &expected_dict);
    if actual_set != expected_set {
        return Err(format!(
            "update result graph mismatch: expected {} triples, got {}",
            expected_set.len(),
            actual_set.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-test execution
// ---------------------------------------------------------------------------

fn compare_update_store(
    loaded: &Loaded,
    default_expected: Option<&Path>,
    named_expected: &[NamedGraphFile],
) -> Result<(), String> {
    if let Some(path) = default_expected {
        let actual_triples = loaded.repo.all_in_txn(None);
        let actual_set = graph_set(&actual_triples, &loaded.dict);
        let (expected_triples, expected_dict) = parse_graph_file(path)?;
        let expected_set = graph_set(&expected_triples, &expected_dict);
        if actual_set != expected_set {
            return Err(format!(
                "update default-graph mismatch: expected {} triples, got {}",
                expected_set.len(),
                actual_set.len()
            ));
        }
    }
    let mut expected_names: BTreeSet<&str> = BTreeSet::new();
    for ng in named_expected {
        expected_names.insert(ng.name.as_str());
        let graph_name = Iri::new(ng.name.clone());
        let actual_quads = loaded.quads.by_graph_name(&graph_name);
        let actual_triples: Vec<Triple> = actual_quads.iter().map(|q| q.triple.clone()).collect();
        let actual_set = graph_set(&actual_triples, &loaded.dict);
        let (expected_triples, expected_dict) = parse_graph_file(&ng.file)?;
        let expected_set = graph_set(&expected_triples, &expected_dict);
        if actual_set != expected_set {
            return Err(format!(
                "update graph {} mismatch: expected {} triples, got {}",
                ng.name,
                expected_set.len(),
                actual_set.len()
            ));
        }
    }
    for quad in loaded.quads.all() {
        if let Some(name) = &quad.graph_name
            && !expected_names.contains(name.as_str())
        {
            return Err(format!(
                "update result contains unexpected named graph {}",
                name.as_str()
            ));
        }
    }
    Ok(())
}

fn run_entry(entry: &TestEntry) -> TestOutcome {
    let result = run_entry_inner(entry);
    match result {
        Ok(()) => TestOutcome {
            pass: true,
            reason: None,
        },
        Err(reason) => TestOutcome {
            pass: false,
            reason: Some(reason),
        },
    }
}

fn fail(reason: FailReason) -> Result<(), FailReason> {
    Err(reason)
}

fn run_entry_inner(entry: &TestEntry) -> Result<(), FailReason> {
    let file = entry
        .request_file
        .as_ref()
        .ok_or_else(|| FailReason::Missing("action/query file".to_owned()))?;
    let text = std::fs::read_to_string(file)
        .map_err(|e| FailReason::Missing(format!("read {}: {e}", file.display())))?;

    match entry.kind {
        TestKind::PositiveSyntax => {
            if parse_only(&text).is_err() {
                return fail(FailReason::ParseFailed);
            }
            Ok(())
        }
        TestKind::NegativeSyntax => {
            if parse_only(&text).is_ok() {
                return fail(FailReason::AcceptedInvalidSyntax);
            }
            Ok(())
        }
        TestKind::QueryEvaluation => {
            let loaded = load_data(&entry.data_files, &entry.graph_data, true)
                .map_err(FailReason::DataFormat)?;
            let actual = execute_query(&loaded, &text).map_err(|e| classify_query_error(&e))?;
            if actual.timed_out {
                return fail(FailReason::Timeout);
            }
            match &entry.expected {
                Expected::Boolean(expected) => {
                    compare_boolean(&actual, *expected).map_err(FailReason::Semantic)
                }
                Expected::Table(path) => {
                    let expected_text = std::fs::read_to_string(path)
                        .map_err(|e| FailReason::ResultFormat(e.to_string()))?;

                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    let (vars, rows, boolean) = match ext {
                        "srx" | "xml" => {
                            parse_srx(&expected_text).map_err(FailReason::ResultFormat)?
                        }
                        "srj" | "json" => {
                            parse_srj(&expected_text).map_err(FailReason::ResultFormat)?
                        }
                        "tsv" => {
                            let (v, r) =
                                parse_tsv(&expected_text).map_err(FailReason::ResultFormat)?;
                            (v, r, None)
                        }
                        "csv" => {
                            let (v, r) =
                                parse_csv(&expected_text).map_err(FailReason::ResultFormat)?;
                            (v, r, None)
                        }
                        _ => {
                            return fail(FailReason::ResultFormat(format!(
                                "unsupported result extension: {ext}"
                            )));
                        }
                    };
                    if let Some(expected_bool) = boolean {
                        return compare_boolean(&actual, expected_bool)
                            .map_err(FailReason::Semantic);
                    }
                    compare_table(&actual, &vars, &rows, &loaded).map_err(FailReason::Semantic)
                }
                Expected::Graph(path) => {
                    // W3C result tables serialized as RDF (`rs:ResultSet`,
                    // aggregates/agg-empty-group-count-graph.ttl) compare as
                    // tables despite the `.ttl` extension.
                    if let Ok((vars, rows)) = parse_rdf_result_set(path) {
                        compare_table(&actual, &vars, &rows, &loaded).map_err(FailReason::Semantic)
                    } else {
                        compare_graph(&actual, path, &loaded).map_err(FailReason::Semantic)
                    }
                }
                Expected::None => Ok(()),
                other => fail(FailReason::Other(format!(
                    "unexpected expected-result shape for query: {other:?}"
                ))),
            }
        }
        TestKind::UpdateEvaluation => {
            let loaded = load_data(&entry.data_files, &entry.graph_data, false)
                .map_err(FailReason::DataFormat)?;
            let actual = execute_query(&loaded, &text).map_err(|e| classify_query_error(&e))?;
            if actual.timed_out {
                return fail(FailReason::Timeout);
            }
            if actual.kind != QueryKind::Update {
                return fail(FailReason::Semantic(format!(
                    "expected UPDATE, got {:?}",
                    actual.kind
                )));
            }
            match &entry.expected {
                Expected::UpdateStore { default, named } => {
                    compare_update_store(&loaded, default.as_deref(), named)
                        .map_err(FailReason::Semantic)
                }
                Expected::Graph(path) => {
                    compare_update_graph(&loaded, path).map_err(FailReason::Semantic)
                }
                Expected::None => Ok(()),
                other => fail(FailReason::Other(format!(
                    "unexpected expected-result shape for update: {other:?}"
                ))),
            }
        }
    }
}

fn classify_query_error(err: &str) -> FailReason {
    let lower = err.to_ascii_lowercase();
    if lower.contains("unsupported") {
        FailReason::Unsupported(err.to_owned())
    } else if lower.contains("timeout") {
        FailReason::Timeout
    } else if lower.contains("parse") || lower.contains("syntax") {
        FailReason::ParseFailed
    } else {
        FailReason::Other(err.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Profile lock + main gate
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

fn load_profile(path: &Path) -> BTreeMap<(String, String), (bool, String)> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let mut parts = line.split('\t');
        let feature = parts.next().unwrap_or("").to_owned();
        let name = parts.next().unwrap_or("").to_owned();
        let outcome = parts.next().unwrap_or("");
        let reason = parts.next().unwrap_or("").to_owned();
        if feature.is_empty() || name.is_empty() {
            continue;
        }
        out.insert((feature, name), (outcome == "PASS", reason));
    }
    out
}

fn write_profile(path: &Path, outcomes: &BTreeMap<(String, String), TestOutcome>) {
    let mut lines = Vec::new();
    for ((feature, name), outcome) in outcomes {
        let outcome_str = if outcome.pass { "PASS" } else { "FAIL" };
        let reason = outcome
            .reason
            .as_ref()
            .map(|r| r.code().to_owned())
            .unwrap_or_default();
        lines.push(format!("{feature}\t{name}\t{outcome_str}\t{reason}"));
    }
    lines.sort();
    let mut out = String::from(
        "# W3C SPARQL 1.1 suite profile (generated by tests/w3c11_suite.rs).\n\
         # feature<TAB>name<TAB>PASS|FAIL<TAB>reason-code\n",
    );
    out.push_str(&lines.join("\n"));
    out.push('\n');
    std::fs::write(path, out).expect("write w3c11 profile");
}

#[test]
fn w3c11_manifest_suite() {
    let suite_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c11");
    let profile_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c11_profile.tsv");
    let learn = env_flag("ONTOLITH_W3C11_LEARN");

    let entries = collect_entries(&suite_root);
    assert!(
        !entries.is_empty(),
        "no W3C SPARQL 1.1 entries discovered under {}",
        suite_root.display()
    );

    let mut outcomes: BTreeMap<(String, String), TestOutcome> = BTreeMap::new();
    for entry in &entries {
        let outcome = run_entry(entry);
        println!(
            "[w3c11] {} / {} :: {} {}",
            entry.feature,
            entry.name,
            if outcome.pass { "PASS" } else { "FAIL" },
            outcome.reason.as_ref().map(|r| r.code()).unwrap_or("")
        );
        outcomes.insert((entry.feature.clone(), entry.name.clone()), outcome);
    }

    if learn {
        write_profile(&profile_path, &outcomes);
        println!(
            "[w3c11] wrote profile {} ({} entries)",
            profile_path.display(),
            outcomes.len()
        );
        return;
    }

    let profile = load_profile(&profile_path);
    assert!(
        !profile.is_empty(),
        "missing profile {}; run: ONTOLITH_W3C11_LEARN=1 cargo test -p ontolith-compliance --test w3c11_suite",
        profile_path.display()
    );

    let mut drift = Vec::new();
    let mut missing = Vec::new();
    for (key, outcome) in &outcomes {
        let Some((expected_pass, _)) = profile.get(key) else {
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
        "[w3c11 summary] total={} pass(must-pass)={} fail(known-gap)={} drift={} missing={}",
        outcomes.len(),
        pass_count,
        fail_count,
        drift.len(),
        missing.len()
    );

    for key in &missing {
        eprintln!("[w3c11] missing profile entry: {}/{}", key.0, key.1);
    }
    for (key, expected, actual, reason) in &drift {
        eprintln!(
            "[w3c11] drift {}/{}: expected {} got {} ({})",
            key.0,
            key.1,
            if *expected { "PASS" } else { "FAIL" },
            if *actual { "PASS" } else { "FAIL" },
            reason
        );
    }

    assert!(
        missing.is_empty(),
        "{} suite entries missing from profile (regenerate with ONTOLITH_W3C11_LEARN=1)",
        missing.len()
    );
    assert!(
        drift.is_empty(),
        "{} profile drifts: regress (or regenerate profile after implementing features)",
        drift.len()
    );
}
