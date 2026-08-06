use ontolith_query::domain::QueryPlanId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceMode {
    Off,
    ForwardChaining,
    Hybrid,
}

impl InferenceMode {
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// RDFS/OWL RL rule identifiers supported by the forward-chaining engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// rdfs:subClassOf is transitive (RDFS 5 / OWL RL prp-trp).
    SubClassOfTransitive,
    /// rdfs:subPropertyOf is transitive (RDFS 6).
    SubPropertyOfTransitive,
    /// p rdfs:subPropertyOf q ∧ x p y → x q y (RDFS 9 / prp-spo1).
    SubPropertyOf,
    /// p rdfs:domain C ∧ x p y → x rdf:type C (RDFS 7 / prp-dom).
    Domain,
    /// p rdfs:range C ∧ x p y → y rdf:type C (RDFS 8 / prp-rng).
    Range,
    /// p owl:inverseOf q ∧ x p y → y q x (prp-inv1).
    InverseOf,
}

impl Rule {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubClassOfTransitive => "rdfs5",
            Self::SubPropertyOfTransitive => "rdfs6",
            Self::SubPropertyOf => "rdfs9",
            Self::Domain => "rdfs7",
            Self::Range => "rdfs8",
            Self::InverseOf => "prp-inv1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningTask {
    pub plan_id: Option<QueryPlanId>,
    pub mode: InferenceMode,
    pub max_iterations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasoningReport {
    pub inferred_triples: usize,
    pub elapsed_ms: u64,
}

/// Outcome of a materialization run.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializeOutcome {
    pub triples: Vec<ontolith_rdf::domain::Triple>,
    pub report: ReasoningReport,
}

pub fn status() -> &'static str {
    "domain"
}
