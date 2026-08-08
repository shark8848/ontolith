//! SHACL validation domain model (L6 — constraint subset).
//!
//! Baseline subset per PLAN-0001 WBS-05: node shapes + property shapes with
//! targets, value/path constraints, severities and validation reports.

use ontolith_rdf::domain::Term;

pub const SHACL_NS: &str = "http://www.w3.org/ns/shacl#";

pub fn shacl(name: &str) -> String {
    format!("{SHACL_NS}{name}")
}

/// Result severity of a validation result (`sh:severity`). Custom severity
/// IRIs (anything other than `sh:Violation`/`sh:Warning`/`sh:Info`) are
/// preserved verbatim so validation reports round-trip them (P6-02).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Violation,
    Warning,
    Info,
    /// Custom severity IRI (`sh:severity <custom-iri>`).
    Custom(String),
}

impl Severity {
    pub fn iri(self) -> String {
        match self {
            Self::Violation => shacl("Violation"),
            Self::Warning => shacl("Warning"),
            Self::Info => shacl("Info"),
            Self::Custom(iri) => iri,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Violation => "Violation",
            Self::Warning => "Warning",
            Self::Info => "Info",
            Self::Custom(_) => "Custom",
        }
    }
}

/// SHACL property path expression (`sh:path`), per SHACL 1.0 §4.3.4. A path
/// is either a plain predicate IRI or a blank node carrying one of the
/// `sh:*Path` predicates (or an RDF list for sequences). Path semantics match
/// SPARQL 1.1 property paths, with set (deduplicated) results.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PropertyPath {
    /// Plain predicate IRI.
    Predicate(String),
    /// `sh:inversePath` — inverse of the inner path.
    Inverse(Box<PropertyPath>),
    /// RDF list of paths — sequence (`p1/p2`).
    Sequence(Vec<PropertyPath>),
    /// `sh:alternativePath` — union (`p1|p2`).
    Alternative(Vec<PropertyPath>),
    /// `sh:zeroOrMorePath` — reflexive transitive closure (`p*`).
    ZeroOrMore(Box<PropertyPath>),
    /// `sh:oneOrMorePath` — transitive closure (`p+`).
    OneOrMore(Box<PropertyPath>),
    /// `sh:zeroOrOnePath` — identity or one step (`p?`).
    ZeroOrOne(Box<PropertyPath>),
}

impl PropertyPath {
    /// Canonical string serialization shared by the engine and the W3C suite
    /// harness for `sh:resultPath` comparison.
    pub fn canonical(&self) -> String {
        match self {
            Self::Predicate(iri) => iri.clone(),
            Self::Inverse(inner) => format!("^{}", parenthesize(inner)),
            Self::Sequence(steps) => steps
                .iter()
                .map(parenthesize)
                .collect::<Vec<_>>()
                .join("/"),
            Self::Alternative(branches) => branches
                .iter()
                .map(parenthesize)
                .collect::<Vec<_>>()
                .join("|"),
            Self::ZeroOrMore(inner) => format!("{}*", parenthesize(inner)),
            Self::OneOrMore(inner) => format!("{}+", parenthesize(inner)),
            Self::ZeroOrOne(inner) => format!("{}?", parenthesize(inner)),
        }
    }

    /// All leaf predicate IRIs of the path (used by `sh:closed`).
    pub fn predicates(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_predicates(&mut out);
        out
    }

    fn collect_predicates(&self, out: &mut Vec<String>) {
        match self {
            Self::Predicate(iri) => out.push(iri.clone()),
            Self::Inverse(inner)
            | Self::ZeroOrMore(inner)
            | Self::OneOrMore(inner)
            | Self::ZeroOrOne(inner) => inner.collect_predicates(out),
            Self::Sequence(steps) => {
                for p in steps {
                    p.collect_predicates(out);
                }
            }
            Self::Alternative(branches) => {
                for p in branches {
                    p.collect_predicates(out);
                }
            }
        }
    }
}

fn parenthesize(p: &PropertyPath) -> String {
    match p {
        PropertyPath::Predicate(_) => p.canonical(),
        _ => format!("({})", p.canonical()),
    }
}

/// RDF node kind required by `sh:nodeKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Iri,
    BlankNode,
    Literal,
    BlankNodeOrIri,
    IriOrLiteral,
    BlankNodeOrLiteral,
}

/// SHACL constraint components in the baseline subset.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintComponent {
    /// `sh:class C` — every value node is a SHACL instance of C.
    Class(String),
    /// `sh:datatype D` — every value node is a literal with datatype D.
    Datatype(String),
    /// `sh:nodeKind K` — every value node has the given RDF node kind.
    NodeKind(NodeKind),
    /// `sh:minCount n` — at least n values (property shapes only).
    MinCount(usize),
    /// `sh:maxCount n` — at most n values (property shapes only).
    MaxCount(usize),
    /// `sh:minLength n` — literal string length >= n.
    MinLength(usize),
    /// `sh:maxLength n` — literal string length <= n.
    MaxLength(usize),
    /// `sh:pattern re` — literal string matches the regex.
    Pattern(String),
    /// `sh:in (v1 … vn)` — value node appears in the given list.
    In(Vec<Term>),
    /// `sh:hasValue v` — at least one value node equals v.
    HasValue(Term),
    /// `sh:languageIn (t1 … tn)` — every value node is a language-tagged
    /// literal whose tag (case-insensitive) is one of the allowed tags.
    LanguageIn(Vec<String>),
    /// `sh:uniqueLang true` — no two value nodes share the same language tag.
    UniqueLang,
    /// `sh:node S` — every value node conforms to the referenced shape.
    Node(String),
    /// `sh:and (S1 … Sn)` — every value node conforms to all listed shapes.
    And(Vec<String>),
    /// `sh:or (S1 … Sn)` — every value node conforms to at least one listed shape.
    Or(Vec<String>),
    /// `sh:xone (S1 … Sn)` — every value node conforms to exactly one listed shape.
    Xone(Vec<String>),
    /// `sh:not S` — no value node conforms to the referenced shape.
    Not(String),
    /// `sh:minInclusive v` — every value node is >= v (numeric or string literal).
    MinInclusive(Term),
    /// `sh:maxInclusive v` — every value node is <= v.
    MaxInclusive(Term),
    /// `sh:minExclusive v` — every value node is > v.
    MinExclusive(Term),
    /// `sh:maxExclusive v` — every value node is < v.
    MaxExclusive(Term),
    /// `sh:equals p` — the value set equals the value set of predicate p (property shapes only).
    Equals(String),
    /// `sh:disjoint p` — the value set is disjoint from the value set of p (property shapes only).
    Disjoint(String),
    /// `sh:lessThan p` — every value node is less than every value node of p (property shapes only).
    LessThan(String),
    /// `sh:lessThanOrEquals p` — every value node is <= every value node of p (property shapes only).
    LessThanOrEquals(String),
    /// `sh:flags f` — regex flags for a sibling `sh:pattern` (e.g. "i").
    PatternFlags(String),
    /// `sh:qualifiedValueShape S` — value nodes must conform to S (property shapes only).
    QualifiedValueShape { shape: String },
    /// `sh:qualifiedMinCount n` — at least n value nodes conform to `sh:qualifiedValueShape`.
    QualifiedMinCount(usize),
    /// `sh:qualifiedMaxCount n` — at most n value nodes conform to `sh:qualifiedValueShape`.
    QualifiedMaxCount(usize),
    /// `sh:qualifiedValueShapesDisjoint true` — values also matching sibling qualified shapes are excluded from counting.
    QualifiedValueShapesDisjoint,
    /// `sh:closed true` — only predicates listed via `sh:property` are allowed.
    Closed,
}

impl ConstraintComponent {
    pub fn component_iri(&self) -> String {
        match self {
            Self::Class(_) => shacl("ClassConstraintComponent"),
            Self::Datatype(_) => shacl("DatatypeConstraintComponent"),
            Self::NodeKind(_) => shacl("NodeKindConstraintComponent"),
            Self::MinCount(_) => shacl("MinCountConstraintComponent"),
            Self::MaxCount(_) => shacl("MaxCountConstraintComponent"),
            Self::MinLength(_) => shacl("MinLengthConstraintComponent"),
            Self::MaxLength(_) => shacl("MaxLengthConstraintComponent"),
            Self::Pattern(_) => shacl("PatternConstraintComponent"),
            Self::In(_) => shacl("InConstraintComponent"),
            Self::HasValue(_) => shacl("HasValueConstraintComponent"),
            Self::LanguageIn(_) => shacl("LanguageInConstraintComponent"),
            Self::UniqueLang => shacl("UniqueLangConstraintComponent"),
            Self::Node(_) => shacl("NodeConstraintComponent"),
            Self::And(_) => shacl("AndConstraintComponent"),
            Self::Or(_) => shacl("OrConstraintComponent"),
            Self::Xone(_) => shacl("XoneConstraintComponent"),
            Self::Not(_) => shacl("NotConstraintComponent"),
            Self::MinInclusive(_) => shacl("MinInclusiveConstraintComponent"),
            Self::MaxInclusive(_) => shacl("MaxInclusiveConstraintComponent"),
            Self::MinExclusive(_) => shacl("MinExclusiveConstraintComponent"),
            Self::MaxExclusive(_) => shacl("MaxExclusiveConstraintComponent"),
            Self::Equals(_) => shacl("EqualsConstraintComponent"),
            Self::Disjoint(_) => shacl("DisjointConstraintComponent"),
            Self::LessThan(_) => shacl("LessThanConstraintComponent"),
            Self::LessThanOrEquals(_) => shacl("LessThanOrEqualsConstraintComponent"),
            Self::PatternFlags(_) => shacl("PatternConstraintComponent"),
            Self::QualifiedValueShape { .. } => shacl("QualifiedValueShapeConstraintComponent"),
            Self::QualifiedMinCount(_) => shacl("QualifiedMinCountConstraintComponent"),
            Self::QualifiedMaxCount(_) => shacl("QualifiedMaxCountConstraintComponent"),
            Self::QualifiedValueShapesDisjoint => shacl("QualifiedValueShapeConstraintComponent"),
            Self::Closed => shacl("ClosedConstraintComponent"),
        }
    }
}

/// Target selector of a shape (`sh:target*`).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// `sh:targetClass C` — all instances of C (direct rdf:type).
    Class(String),
    /// `sh:targetNode n` — the specific node.
    Node(Term),
    /// `sh:targetSubjectsOf p` — all subjects with predicate p.
    SubjectsOf(String),
    /// `sh:targetObjectsOf p` — all objects with predicate p.
    ObjectsOf(String),
}

/// One `sh:property` nested property shape.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyShape {
    /// Shape subject key (IRI or blank-node label) — reported as
    /// `sh:sourceShape` for property-shape constraint results.
    pub id: String,
    /// `sh:path` property path.
    pub path: PropertyPath,
    pub constraints: Vec<ConstraintComponent>,
    /// Nested `sh:property` shapes: applied to the values of `path` as focus
    /// nodes (property shape nesting, e.g. `validation-reports/shared`).
    pub property_shapes: Vec<PropertyShape>,
    pub severity: Severity,
    pub message: Option<String>,
}

/// A parsed node shape (or standalone property shape).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Shape subject key: IRI string or blank-node label.
    pub id: String,
    pub targets: Vec<Target>,
    /// `Some(path)` when the shape itself carries `sh:path` (a standalone
    /// property shape): its constraints apply to the values of the path.
    pub path: Option<PropertyPath>,
    pub constraints: Vec<ConstraintComponent>,
    pub property_shapes: Vec<PropertyShape>,
    /// `sh:ignoredProperties` — extra predicates allowed by `sh:closed`.
    pub ignored_properties: Vec<String>,
    /// `sh:deactivated true` — the shape is skipped during validation.
    pub deactivated: bool,
    pub severity: Severity,
    pub message: Option<String>,
}

/// One violation/warning/info item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub focus_node: String,
    pub path: Option<String>,
    pub value: Option<String>,
    pub source_shape: Option<String>,
    pub component: String,
    pub severity: Severity,
    pub message: Option<String>,
}

/// SHACL validation report (`sh:ValidationReport`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationReport {
    pub conforms: bool,
    pub results: Vec<ValidationResult>,
}
