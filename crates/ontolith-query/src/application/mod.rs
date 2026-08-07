//! Query application contracts (L3).

use crate::domain::{
    QueryExplain, QueryPlan, QueryRequest, QueryResult, QueryResultSummary, TriplePattern,
};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_storage::application::StorageEngine;
use ontolith_storage::domain::{WriteBatch, WriteOperation};
use ontolith_transaction::domain::TxnId;
use std::sync::Arc;

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

    /// Dictionary-backed IRI → NodeId encoding for INSERT DATA subjects.
    /// Returns `None` when no dictionary bridge is available.
    fn encode_node(&self, _value: &str) -> Option<NodeId> {
        None
    }

    /// Dictionary-backed NodeId → value decode (IRI strings or `_:label`
    /// blank labels). Returns `None` when no dictionary bridge is available.
    fn decode_node(&self, _node_id: NodeId) -> Option<String> {
        None
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

    /// Named-graph triples of one graph (GRAPH patterns / WITH / USING datasets).
    fn quads_in_graph(&self, _graph: &Iri, _txn_id: Option<TxnId>) -> Vec<Triple> {
        Vec::new()
    }

    /// Distinct named-graph names in the store (for `GRAPH ?var { ... }`).
    fn named_graph_names(&self, _txn_id: Option<TxnId>) -> Vec<Iri> {
        Vec::new()
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

/// Cardinality statistics used by the cost-based optimizer (P3-02).
pub trait QueryStatistics: Send + Sync {
    fn triple_count(&self) -> u64;

    fn distinct_subjects(&self) -> u64;

    fn distinct_predicates(&self) -> u64;

    fn distinct_objects(&self) -> u64;

    /// Uniform-selectivity estimate for one triple pattern: the product of
    /// per-position bound selectivities (distinct values / total triples),
    /// clamped to `[1e-9, 1]`.
    fn pattern_selectivity(&self, pattern: &TriplePattern) -> f64 {
        let total = self.triple_count().max(1) as f64;
        let mut sel = 1.0;
        if !pattern.subject.is_variable() {
            sel *= self.distinct_subjects().max(1) as f64 / total;
        }
        if !pattern.predicate.is_variable() {
            sel *= self.distinct_predicates().max(1) as f64 / total;
        }
        if !pattern.object.is_variable() {
            sel *= self.distinct_objects().max(1) as f64 / total;
        }
        sel.clamp(1e-9, 1.0)
    }
}

/// Write surface required by the SPARQL Update executor (L3).
pub trait UpdateWriteService: Send + Sync {
    fn apply_write_batch(
        &self,
        txn_id: TxnId,
        operations: Vec<WriteOperation>,
    ) -> Result<(), OntolithError>;

    fn commit(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    fn abort(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    /// Default-graph triples as of `txn_id` (for CLEAR/DROP DEFAULT).
    fn default_graph_triples(&self, txn_id: Option<TxnId>) -> Vec<Triple>;

    /// All named-graph quads (for CLEAR/DROP NAMED/ALL), as of `txn_id`
    /// including staged (uncommitted) writes.
    fn named_graph_quads(&self) -> Vec<Quad> {
        self.named_graph_quads_in_txn(None)
    }

    /// All named-graph quads as of `txn_id`, including staged writes.
    fn named_graph_quads_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Quad> {
        let _ = txn_id;
        self.named_graph_quads()
    }

    /// Quads of one named graph (for CLEAR/DROP GRAPH and LOAD), as of
    /// `txn_id` including staged writes.
    fn quads_in_graph(&self, graph: &Iri, txn_id: Option<TxnId>) -> Vec<Quad> {
        self.named_graph_quads_in_txn(txn_id)
            .into_iter()
            .filter(|q| q.graph_name.as_ref() == Some(graph))
            .collect()
    }
}

/// [`UpdateWriteService`] backed by a [`StorageEngine`] (memory or RocksDB).
pub struct EngineUpdateWriteService {
    engine: Arc<dyn StorageEngine>,
}

impl EngineUpdateWriteService {
    pub fn new(engine: Arc<dyn StorageEngine>) -> Self {
        Self { engine }
    }
}

impl UpdateWriteService for EngineUpdateWriteService {
    fn apply_write_batch(
        &self,
        txn_id: TxnId,
        operations: Vec<WriteOperation>,
    ) -> Result<(), OntolithError> {
        self.engine
            .apply_write_batch(&WriteBatch { txn_id, operations })
    }

    fn commit(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        self.engine.commit_transaction(txn_id)
    }

    fn abort(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        self.engine.abort_transaction(txn_id)
    }

    fn default_graph_triples(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.default_graph_triples_in_txn(txn_id)
    }

    fn named_graph_quads(&self) -> Vec<Quad> {
        self.engine.named_graph_quads()
    }

    fn named_graph_quads_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Quad> {
        self.engine.named_graph_quads_in_txn(txn_id)
    }

    fn quads_in_graph(&self, graph: &Iri, txn_id: Option<TxnId>) -> Vec<Quad> {
        self.engine.quads_by_graph_in_txn(Some(graph), txn_id)
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
