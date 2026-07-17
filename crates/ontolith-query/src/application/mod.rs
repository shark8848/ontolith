use crate::domain::{QueryRequest, QueryResultSummary};
use ontolith_core::domain::NodeId;
use ontolith_core::error::OntolithError;

pub trait QueryReadService: Send + Sync {
    fn execute_select_all(
        &self,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError>;

    fn execute_select_by_subject(
        &self,
        request: &QueryRequest,
        subject: NodeId,
    ) -> Result<QueryResultSummary, OntolithError>;
}

pub trait QueryPlanner: Send + Sync {
    fn plan(&self, request: &QueryRequest) -> Result<crate::domain::QueryPlan, OntolithError>;
}

pub trait QueryExecutor: Send + Sync {
    fn execute(
        &self,
        plan: &crate::domain::QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError>;
}

pub struct QueryPipeline<P: QueryPlanner, E: QueryExecutor> {
    planner: P,
    executor: E,
}

impl<P: QueryPlanner, E: QueryExecutor> QueryPipeline<P, E> {
    pub fn new(planner: P, executor: E) -> Self {
        Self { planner, executor }
    }

    pub fn execute(&self, request: &QueryRequest) -> Result<QueryResultSummary, OntolithError> {
        let plan = self.planner.plan(request)?;
        self.executor.execute(&plan, request)
    }
}

pub fn status() -> &'static str {
    "application"
}
