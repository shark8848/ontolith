use ontolith_transaction::domain::TxnId;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryPlanId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPlan {
    pub id: QueryPlanId,
    pub kind: QueryKind,
    pub logical_steps: Vec<String>,
    pub physical_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub query: QueryText,
    pub txn_id: Option<TxnId>,
    pub tenant: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryResultSummary {
    pub row_count: usize,
    pub elapsed_ms: u64,
}

pub fn status() -> &'static str {
    "domain"
}
