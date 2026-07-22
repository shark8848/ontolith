//! Query domain model (L3 — full SPARQL 1.1 Query core surface).

use ontolith_core::domain::{ConsistencyLevel, Iri, LiteralValue, NodeId};
use ontolith_rdf::domain::Term;
use ontolith_transaction::domain::TxnId;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryText(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    Select,
    Construct,
    Ask,
    Describe,
    Update,
}

impl QueryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Construct => "CONSTRUCT",
            Self::Ask => "ASK",
            Self::Describe => "DESCRIBE",
            Self::Update => "UPDATE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryPlanId(pub u64);

/// RDF term or variable in a pattern position.
#[derive(Debug, Clone, PartialEq)]
pub enum TermPattern {
    Variable(String),
    Node(NodeId),
    Iri(Iri),
    Literal(LiteralValue),
    /// Blank node label in query (treated as existential var for BGP matching).
    Blank(String),
}

impl TermPattern {
    pub fn as_variable(&self) -> Option<&str> {
        match self {
            Self::Variable(v) | Self::Blank(v) => Some(v.as_str()),
            _ => None,
        }
    }

    pub fn is_variable(&self) -> bool {
        self.as_variable().is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TriplePattern {
    pub subject: TermPattern,
    pub predicate: TermPattern,
    pub object: TermPattern,
}

/// FILTER / BIND expression subset.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Variable(String),
    Iri(Iri),
    Literal(LiteralValue),
    Bound(String),
    IsIri(Box<Expression>),
    IsLiteral(Box<Expression>),
    IsBlank(Box<Expression>),
    Not(Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Equal(Box<Expression>, Box<Expression>),
    NotEqual(Box<Expression>, Box<Expression>),
    Less(Box<Expression>, Box<Expression>),
    LessEq(Box<Expression>, Box<Expression>),
    Greater(Box<Expression>, Box<Expression>),
    GreaterEq(Box<Expression>, Box<Expression>),
}

/// Aggregate function subset for SELECT projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateFunction {
    /// COUNT(*) when `variable` is None, COUNT(?v) otherwise.
    Count { variable: Option<String> },
}

/// Property path subset used by the L3 executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathExpression {
    Predicate(Iri),
    InversePredicate(Iri),
    Sequence(Box<PathExpression>, Box<PathExpression>),
    Alternative(Box<PathExpression>, Box<PathExpression>),
    OneOrMore(Box<PathExpression>),
    ZeroOrMore(Box<PathExpression>),
}

/// SPARQL algebra (W3C-style subset used by the executor).
#[derive(Debug, Clone, PartialEq)]
pub enum Algebra {
    Bgp(Vec<TriplePattern>),
    Join {
        left: Box<Algebra>,
        right: Box<Algebra>,
    },
    LeftJoin {
        left: Box<Algebra>,
        right: Box<Algebra>,
        condition: Option<Expression>,
    },
    Union {
        left: Box<Algebra>,
        right: Box<Algebra>,
    },
    Filter {
        expression: Expression,
        input: Box<Algebra>,
    },
    Extend {
        variable: String,
        expression: Expression,
        input: Box<Algebra>,
    },
    /// VALUES clause: list of variables + rows of bound terms (None = UNDEF).
    Values {
        variables: Vec<String>,
        bindings: Vec<Vec<Option<TermPattern>>>,
    },
    Distinct {
        input: Box<Algebra>,
    },
    Project {
        variables: Vec<String>,
        input: Box<Algebra>,
    },
    OrderBy {
        keys: Vec<OrderKey>,
        input: Box<Algebra>,
    },
    Slice {
        offset: usize,
        limit: Option<usize>,
        input: Box<Algebra>,
    },
    Aggregate {
        function: AggregateFunction,
        output: String,
        input: Box<Algebra>,
    },
    Path {
        subject: TermPattern,
        path: PathExpression,
        object: TermPattern,
    },
    /// Empty identity multiset (one empty solution) — unit for joins.
    Identity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderKey {
    pub variable: String,
    pub ascending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryPlan {
    pub id: QueryPlanId,
    pub kind: QueryKind,
    pub algebra: Algebra,
    pub prefixes: BTreeMap<String, String>,
    pub base: Option<String>,
    pub logical_steps: Vec<String>,
    pub physical_steps: Vec<String>,
    /// CONSTRUCT template (only for Construct).
    pub construct_template: Vec<TriplePattern>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryExplain {
    pub plan_id: QueryPlanId,
    pub kind: QueryKind,
    pub logical_steps: Vec<String>,
    pub physical_steps: Vec<String>,
    pub algebra_summary: String,
}

impl QueryPlan {
    pub fn explain(&self) -> QueryExplain {
        QueryExplain {
            plan_id: self.id,
            kind: self.kind,
            logical_steps: self.logical_steps.clone(),
            physical_steps: self.physical_steps.clone(),
            algebra_summary: summarize_algebra(&self.algebra),
        }
    }
}

pub fn summarize_algebra(algebra: &Algebra) -> String {
    match algebra {
        Algebra::Identity => "Identity".into(),
        Algebra::Bgp(p) => format!("Bgp({})", p.len()),
        Algebra::Join { left, right } => {
            format!(
                "Join({}, {})",
                summarize_algebra(left),
                summarize_algebra(right)
            )
        }
        Algebra::LeftJoin {
            left,
            right,
            condition,
        } => format!(
            "LeftJoin({}, {}, cond={})",
            summarize_algebra(left),
            summarize_algebra(right),
            condition.is_some()
        ),
        Algebra::Union { left, right } => {
            format!(
                "Union({}, {})",
                summarize_algebra(left),
                summarize_algebra(right)
            )
        }
        Algebra::Filter { input, .. } => format!("Filter({})", summarize_algebra(input)),
        Algebra::Extend {
            variable, input, ..
        } => format!("Extend({variable}, {})", summarize_algebra(input)),
        Algebra::Values {
            variables,
            bindings,
        } => format!("Values(vars={}, rows={})", variables.len(), bindings.len()),
        Algebra::Distinct { input } => format!("Distinct({})", summarize_algebra(input)),
        Algebra::Project { variables, input } => {
            let v = if variables.is_empty() {
                "*".into()
            } else {
                variables.join(",")
            };
            format!("Project({v}, {})", summarize_algebra(input))
        }
        Algebra::OrderBy { keys, input } => {
            format!("OrderBy(keys={}, {})", keys.len(), summarize_algebra(input))
        }
        Algebra::Slice {
            offset,
            limit,
            input,
        } => format!(
            "Slice(offset={offset}, limit={limit:?}, {})",
            summarize_algebra(input)
        ),
        Algebra::Aggregate {
            function,
            output,
            input,
        } => {
            let fun = match function {
                AggregateFunction::Count { variable: None } => "COUNT(*)".to_string(),
                AggregateFunction::Count { variable: Some(v) } => format!("COUNT(?{v})"),
            };
            format!("Aggregate({fun} AS ?{output}, {})", summarize_algebra(input))
        }
        Algebra::Path {
            subject,
            path,
            object,
        } => format!("Path({subject:?}, {}, {object:?})", summarize_path(path)),
    }
}

fn summarize_path(path: &PathExpression) -> String {
    match path {
        PathExpression::Predicate(p) => format!("<{}>", p.as_str()),
        PathExpression::InversePredicate(p) => format!("^<{}>", p.as_str()),
        PathExpression::Sequence(left, right) => {
            format!("{}/{}", summarize_path(left), summarize_path(right))
        }
        PathExpression::Alternative(left, right) => {
            format!("{}|{}", summarize_path(left), summarize_path(right))
        }
        PathExpression::OneOrMore(inner) => format!("{}+", summarize_path(inner)),
        PathExpression::ZeroOrMore(inner) => format!("{}*", summarize_path(inner)),
    }
}

/// Bound value in a solution mapping.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundValue {
    Node(NodeId),
    Iri(Iri),
    Literal(LiteralValue),
    Blank(NodeId),
}

impl BoundValue {
    pub fn from_term(term: &Term) -> Self {
        match term {
            Term::Iri(i) => Self::Iri(i.clone()),
            Term::BlankNode(n) => Self::Blank(*n),
            Term::Literal(l) => Self::Literal(l.clone()),
        }
    }

    pub fn to_term(&self) -> Term {
        match self {
            Self::Iri(i) => Term::Iri(i.clone()),
            Self::Node(n) | Self::Blank(n) => Term::BlankNode(*n),
            Self::Literal(l) => Term::Literal(l.clone()),
        }
    }
}

/// One SPARQL solution mapping (variable → bound value).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Solution {
    pub bindings: BTreeMap<String, BoundValue>,
}

impl Solution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, var: &str) -> Option<&BoundValue> {
        self.bindings.get(var)
    }

    pub fn insert(&mut self, var: impl Into<String>, value: BoundValue) {
        self.bindings.insert(var.into(), value);
    }

    pub fn merge(&self, other: &Solution) -> Option<Solution> {
        let mut out = self.clone();
        for (k, v) in &other.bindings {
            if let Some(existing) = out.bindings.get(k) {
                if existing != v {
                    return None;
                }
            } else {
                out.bindings.insert(k.clone(), v.clone());
            }
        }
        Some(out)
    }
}

#[derive(Debug, Clone)]
pub struct QueryRequest {
    pub query: QueryText,
    pub txn_id: Option<TxnId>,
    pub tenant: Option<String>,
    pub timeout_ms: Option<u64>,
    /// Cooperative cancellation flag; set true to stop execution.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Client-visible read consistency (SAS-0001 §8); single-node engines treat
    /// Strong/Session equivalently for committed data.
    pub consistency: ConsistencyLevel,
}

impl PartialEq for QueryRequest {
    fn eq(&self, other: &Self) -> bool {
        self.query == other.query
            && self.txn_id == other.txn_id
            && self.tenant == other.tenant
            && self.timeout_ms == other.timeout_ms
            && self.consistency == other.consistency
            && self.is_cancelled() == other.is_cancelled()
    }
}

impl QueryRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: QueryText(query.into()),
            txn_id: None,
            tenant: None,
            timeout_ms: None,
            cancel: None,
            consistency: ConsistencyLevel::Strong,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_txn(mut self, txn_id: TxnId) -> Self {
        self.txn_id = Some(txn_id);
        self
    }

    pub fn with_cancel(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    pub fn with_consistency(mut self, level: ConsistencyLevel) -> Self {
        self.consistency = level;
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub kind: QueryKind,
    pub variables: Vec<String>,
    pub solutions: Vec<Solution>,
    pub boolean: Option<bool>,
    pub construct_triples: Vec<ontolith_rdf::domain::Triple>,
    pub elapsed_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl QueryResult {
    pub fn row_count(&self) -> usize {
        if let Some(b) = self.boolean {
            return usize::from(b);
        }
        if !self.construct_triples.is_empty() {
            return self.construct_triples.len();
        }
        self.solutions.len()
    }
}

/// Backward-compatible summary used by older call sites / metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryResultSummary {
    pub row_count: usize,
    pub elapsed_ms: u64,
    pub timed_out: bool,
}

impl From<&QueryResult> for QueryResultSummary {
    fn from(value: &QueryResult) -> Self {
        Self {
            row_count: value.row_count(),
            elapsed_ms: value.elapsed_ms,
            timed_out: value.timed_out,
        }
    }
}

pub fn status() -> &'static str {
    "domain"
}
