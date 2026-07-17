use ontolith_query::domain::QueryPlanId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceMode {
    Off,
    ForwardChaining,
    Hybrid,
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

pub fn status() -> &'static str {
    "domain"
}
