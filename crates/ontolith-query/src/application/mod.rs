//! Query application contracts (L3).

use crate::domain::{QueryExplain, QueryPlan, QueryRequest, QueryResult, QueryResultSummary};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Term, Triple};

/// Storage-backed triple access used by the physical executor.
pub trait QueryReadService: Send + Sync {
    fn all_triples(
        &self,
        txn_id: Option<ontolith_transaction::domain::TxnId>,
    ) -> Result<Vec<Triple>, OntolithError>;

    fn by_subject(
        &self,
        subject: NodeId,
        txn_id: Option<ontolith_transaction::domain::TxnId>,
    ) -> Result<Vec<Triple>, OntolithError>;

    fn by_predicate(
        &self,
        predicate: &Iri,
        txn_id: Option<ontolith_transaction::domain::TxnId>,
    ) -> Result<Vec<Triple>, OntolithError>;

    fn by_object(
        &self,
        object: &Term,
        txn_id: Option<ontolith_transaction::domain::TxnId>,
    ) -> Result<Vec<Triple>, OntolithError>;

    /// Optional dictionary bridge for features that need subject-node lookup by IRI.
    fn node_for_iri(&self, _iri: &Iri) -> Result<Option<NodeId>, OntolithError> {
        Ok(None)
    }

    /// Multi-bound pattern probe (L2 `matching_in_txn`); default filters single-index results.
    fn matching(
        &self,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<ontolith_transaction::domain::TxnId>,
    ) -> Result<Vec<Triple>, OntolithError> {
        let mut triples = if let Some(s) = subject {
            self.by_subject(s, txn_id)?
        } else if let Some(p) = predicate {
            self.by_predicate(p, txn_id)?
        } else if let Some(o) = object {
            self.by_object(o, txn_id)?
        } else {
            self.all_triples(txn_id)?
        };
        if let Some(p) = predicate {
            triples.retain(|t| &t.predicate == p);
        }
        if let Some(o) = object {
            triples.retain(|t| &t.object == o);
        }
        if let Some(s) = subject {
            triples.retain(|t| t.subject == s);
        }
        Ok(triples)
    }

    /// Legacy summary helpers used by older tests / pipelines.
    fn execute_select_all(
        &self,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started = std::time::Instant::now();
        let rows = self.all_triples(request.txn_id)?.len();
        Ok(QueryResultSummary {
            row_count: rows,
            elapsed_ms: started.elapsed().as_millis() as u64,
            timed_out: false,
        })
    }

    fn execute_select_by_subject(
        &self,
        request: &QueryRequest,
        subject: NodeId,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started = std::time::Instant::now();
        let rows = self.by_subject(subject, request.txn_id)?.len();
        Ok(QueryResultSummary {
            row_count: rows,
            elapsed_ms: started.elapsed().as_millis() as u64,
            timed_out: false,
        })
    }

    fn execute_select_by_predicate(
        &self,
        request: &QueryRequest,
        predicate: &Iri,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started = std::time::Instant::now();
        let rows = self.by_predicate(predicate, request.txn_id)?.len();
        Ok(QueryResultSummary {
            row_count: rows,
            elapsed_ms: started.elapsed().as_millis() as u64,
            timed_out: false,
        })
    }

    fn execute_select_by_object(
        &self,
        request: &QueryRequest,
        object: &Term,
    ) -> Result<QueryResultSummary, OntolithError> {
        let started = std::time::Instant::now();
        let rows = self.by_object(object, request.txn_id)?.len();
        Ok(QueryResultSummary {
            row_count: rows,
            elapsed_ms: started.elapsed().as_millis() as u64,
            timed_out: false,
        })
    }
}

pub trait QueryPlanner: Send + Sync {
    fn plan(&self, request: &QueryRequest) -> Result<QueryPlan, OntolithError>;
}

pub trait QueryOptimizer: Send + Sync {
    fn optimize(&self, plan: QueryPlan) -> Result<QueryPlan, OntolithError>;
}

pub trait QueryExecutor: Send + Sync {
    fn execute(
        &self,
        plan: &QueryPlan,
        request: &QueryRequest,
    ) -> Result<QueryResult, OntolithError>;
}

/// No-op optimizer (identity).
#[derive(Debug, Default, Clone, Copy)]
pub struct IdentityOptimizer;

impl QueryOptimizer for IdentityOptimizer {
    fn optimize(&self, plan: QueryPlan) -> Result<QueryPlan, OntolithError> {
        Ok(plan)
    }
}

pub struct QueryPipeline<P, O, E>
where
    P: QueryPlanner,
    O: QueryOptimizer,
    E: QueryExecutor,
{
    planner: P,
    optimizer: O,
    executor: E,
}

impl<P, O, E> QueryPipeline<P, O, E>
where
    P: QueryPlanner,
    O: QueryOptimizer,
    E: QueryExecutor,
{
    pub fn new(planner: P, optimizer: O, executor: E) -> Self {
        Self {
            planner,
            optimizer,
            executor,
        }
    }

    pub fn plan(&self, request: &QueryRequest) -> Result<QueryPlan, OntolithError> {
        let plan = self.planner.plan(request)?;
        self.optimizer.optimize(plan)
    }

    pub fn explain(&self, request: &QueryRequest) -> Result<QueryExplain, OntolithError> {
        Ok(self.plan(request)?.explain())
    }

    pub fn execute(&self, request: &QueryRequest) -> Result<QueryResult, OntolithError> {
        let plan = self.plan(request)?;
        self.executor.execute(&plan, request)
    }

    /// Backward-compatible summary execute.
    pub fn execute_summary(
        &self,
        request: &QueryRequest,
    ) -> Result<QueryResultSummary, OntolithError> {
        Ok(QueryResultSummary::from(&self.execute(request)?))
    }
}

pub fn status() -> &'static str {
    "application"
}
