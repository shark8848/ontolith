//! SHACL validation domain model (L6 — constraint subset).
//!
//! Baseline subset per PLAN-0001 WBS-05: node shapes + property shapes with
//! targets, value/path constraints, severities and validation reports.

use ontolith_rdf::domain::Term;

pub const SHACL_NS: &str = "http://www.w3.org/ns/shacl#";

pub fn shacl(name: &str) -> String {
    format!("{SHACL_NS}{name}")
}

/// Result severity of a validation result (`sh:severity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Violation,
    Warning,
    Info,
}

impl Severity {
    pub fn iri(self) -> String {
        shacl(match self {
            Self::Violation => "Violation",
            Self::Warning => "Warning",
            Self::Info => "Info",
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Violation => "Violation",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
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
    /// `sh:node S` — every value node conforms to the referenced shape.
    Node(String),
    /// `sh:and (S1 … Sn)` — every value node conforms to all listed shapes.
    And(Vec<String>),
    /// `sh:or (S1 … Sn)` — every value node conforms to at least one listed shape.
    Or(Vec<String>),
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
            Self::Node(_) => shacl("NodeConstraintComponent"),
            Self::And(_) => shacl("AndConstraintComponent"),
            Self::Or(_) => shacl("OrConstraintComponent"),
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
    /// `sh:path` predicate.
    pub path: String,
    pub constraints: Vec<ConstraintComponent>,
    pub severity: Severity,
    pub message: Option<String>,
}

/// A parsed node shape (or standalone property shape).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Shape subject key: IRI string or blank-node label.
    pub id: String,
    pub targets: Vec<Target>,
    pub constraints: Vec<ConstraintComponent>,
    pub property_shapes: Vec<PropertyShape>,
    /// `sh:ignoredProperties` — extra predicates allowed by `sh:closed`.
    pub ignored_properties: Vec<String>,
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
