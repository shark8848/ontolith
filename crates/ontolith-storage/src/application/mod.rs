//! Storage application contracts (L2).
//!
//! All durable engines (in-memory today, RocksDB later) implement these traits.
//! Upper layers must not depend on infrastructure types directly.

use crate::domain::{
    IndexMaintenance, SnapshotRef, StorageKey, StorageStats, WalRecord, WriteBatch,
};
use ontolith_core::domain::{ConsistencyLevel, Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_transaction::application::{TransactionManager, UnitOfWork};
use ontolith_transaction::domain::{TxnId, TxnMode};

/// Bidirectional dictionary: lexical form ↔ stable [`NodeId`].
///
/// IDs are immutable for the lifetime of a dictionary epoch (SAS-0401 §5).
pub trait DictionaryCodec: Send + Sync {
    fn encode_node(&self, value: &str) -> NodeId;

    fn decode_node(&self, node_id: NodeId) -> Option<String>;

    fn len(&self) -> usize {
        0
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn contains_value(&self, value: &str) -> bool {
        let id = self.encode_node(value);
        self.decode_node(id).as_deref() == Some(value)
    }

    fn contains_node(&self, node_id: NodeId) -> bool {
        self.decode_node(node_id).is_some()
    }

    /// Dictionary epoch; increments if the mapping table is replaced/cleared.
    fn epoch(&self) -> u64 {
        0
    }
}

/// Write-ahead log used for staged / committed / aborted durability markers.
pub trait WriteAheadLog: Send + Sync {
    fn append(&self, record: WalRecord) -> Result<(), OntolithError>;

    fn entries(&self) -> Vec<WalRecord>;

    fn truncate_prefix(&self, upto_exclusive: usize) -> Result<(), OntolithError>;
}

/// Core storage engine: transactional writes + multi-index reads.
///
/// ## Write lifecycle
/// 1. `apply_write_batch` — stage ops (WAL Staged); visible only to same txn
/// 2. `commit_transaction` — apply + **incremental** index maintain (WAL Committed)
/// 3. `abort_transaction` — drop pending (WAL Aborted)
///
/// ## Indexes
/// Default graph maintains all six permutations (SPO/SOP/PSO/POS/OSP/OPS).
/// Named graphs maintain a graph→quad secondary index.
///
/// ## Dedup
/// PutTriple/PutQuad are set-semantic: inserting an existing statement is a no-op.
pub trait StorageEngine: Send + Sync {
    fn apply_write_batch(&self, batch: &WriteBatch) -> Result<(), OntolithError>;

    fn commit_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    fn abort_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    fn delete_by_key(&self, key: &StorageKey) -> Result<usize, OntolithError>;

    fn snapshot(&self) -> SnapshotRef {
        self.snapshot_with(ConsistencyLevel::Strong, None)
    }

    fn snapshot_with(
        &self,
        consistency: ConsistencyLevel,
        read_txn_id: Option<TxnId>,
    ) -> SnapshotRef;

    fn stats(&self) -> StorageStats;

    fn index_maintenance(&self) -> IndexMaintenance {
        IndexMaintenance::Sync
    }

    fn default_graph_triples(&self) -> Vec<Triple> {
        self.default_graph_triples_in_txn(None)
    }

    fn default_graph_triples_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn triples_by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn triples_by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn triples_by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple>;

    /// Multi-bound pattern probe using the best available index.
    fn triples_matching_in_txn(
        &self,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<TxnId>,
    ) -> Vec<Triple> {
        // Default: pick the most selective bound position, then filter.
        let mut triples = if let Some(s) = subject {
            self.triples_by_subject_in_txn(s, txn_id)
        } else if let Some(p) = predicate {
            self.triples_by_predicate_in_txn(p, txn_id)
        } else if let Some(o) = object {
            self.triples_by_object_in_txn(o, txn_id)
        } else {
            self.default_graph_triples_in_txn(txn_id)
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
        triples
    }

    fn named_graph_quads(&self) -> Vec<Quad>;

    fn quads_by_graph_in_txn(&self, graph_name: Option<&Iri>, txn_id: Option<TxnId>) -> Vec<Quad> {
        let _ = txn_id;
        match graph_name {
            Some(g) => self
                .named_graph_quads()
                .into_iter()
                .filter(|q| q.graph_name.as_ref() == Some(g))
                .collect(),
            None => self
                .default_graph_triples()
                .into_iter()
                .map(Quad::in_default_graph)
                .collect(),
        }
    }
}

/// Repository façade over default-graph triples.
pub trait TripleRepository: Send + Sync {
    fn insert(&self, txn_id: TxnId, triple: Triple) -> Result<(), OntolithError>;

    fn delete(&self, txn_id: TxnId, triple: Triple) -> Result<(), OntolithError> {
        let _ = (txn_id, triple);
        Err(OntolithError::Unsupported("exact triple delete"))
    }

    fn all_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn matching_in_txn(
        &self,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<TxnId>,
    ) -> Vec<Triple> {
        let mut triples = if let Some(s) = subject {
            self.by_subject_in_txn(s, txn_id)
        } else if let Some(p) = predicate {
            self.by_predicate_in_txn(p, txn_id)
        } else if let Some(o) = object {
            self.by_object_in_txn(o, txn_id)
        } else {
            self.all_in_txn(txn_id)
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
        triples
    }

    fn all(&self) -> Vec<Triple> {
        self.all_in_txn(None)
    }

    fn by_subject(&self, subject: NodeId) -> Vec<Triple> {
        self.by_subject_in_txn(subject, None)
    }

    fn by_predicate(&self, predicate: &Iri) -> Vec<Triple> {
        self.by_predicate_in_txn(predicate, None)
    }

    fn by_object(&self, object: &Term) -> Vec<Triple> {
        self.by_object_in_txn(object, None)
    }
}

/// Repository façade over named-graph quads.
pub trait QuadRepository: Send + Sync {
    fn insert(&self, txn_id: TxnId, quad: Quad) -> Result<(), OntolithError>;

    fn delete(&self, txn_id: TxnId, quad: Quad) -> Result<(), OntolithError> {
        let _ = (txn_id, quad);
        Err(OntolithError::Unsupported("exact quad delete"))
    }

    fn all(&self) -> Vec<Quad>;

    fn by_graph_name(&self, graph_name: &Iri) -> Vec<Quad>;

    fn by_graph_name_in_txn(&self, graph_name: &Iri, txn_id: Option<TxnId>) -> Vec<Quad> {
        let _ = txn_id;
        self.by_graph_name(graph_name)
    }
}

/// Coordinates transaction manager + storage engine for a one-shot write.
pub struct TransactionalWriteService<'a, M: TransactionManager, E: StorageEngine> {
    manager: &'a M,
    engine: &'a E,
}

impl<'a, M: TransactionManager, E: StorageEngine> TransactionalWriteService<'a, M, E> {
    pub fn new(manager: &'a M, engine: &'a E) -> Self {
        Self { manager, engine }
    }

    pub fn commit_write_operations(
        &self,
        mode: TxnMode,
        operations: Vec<crate::domain::WriteOperation>,
    ) -> Result<TxnId, OntolithError> {
        let uow = UnitOfWork::begin(self.manager, mode)?;
        let txn = uow.transaction();
        let batch = WriteBatch {
            txn_id: txn.id,
            operations,
        };

        if let Err(err) = self.engine.apply_write_batch(&batch) {
            let _ = self.engine.abort_transaction(txn.id);
            let _ = uow.abort();
            return Err(err);
        }

        if let Err(err) = self.engine.commit_transaction(txn.id) {
            let _ = self.engine.abort_transaction(txn.id);
            let _ = uow.abort();
            return Err(err);
        }

        if uow.commit().is_err() {
            return Err(OntolithError::InvalidState(
                "transaction manager commit failed after storage commit",
            ));
        }

        Ok(txn.id)
    }
}

pub fn status() -> &'static str {
    "application"
}
