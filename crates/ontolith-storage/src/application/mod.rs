use crate::domain::{SnapshotRef, StorageKey, WalRecord, WriteBatch};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Triple};
use ontolith_transaction::application::{TransactionManager, UnitOfWork};
use ontolith_transaction::domain::{TxnId, TxnMode};

pub trait DictionaryCodec: Send + Sync {
    fn encode_node(&self, value: &str) -> NodeId;

    fn decode_node(&self, node_id: NodeId) -> Option<String>;
}

pub trait WriteAheadLog: Send + Sync {
    fn append(&self, record: WalRecord) -> Result<(), OntolithError>;

    fn entries(&self) -> Vec<WalRecord>;

    fn truncate_prefix(&self, upto_exclusive: usize) -> Result<(), OntolithError>;
}

pub trait StorageEngine: Send + Sync {
    fn apply_write_batch(&self, batch: &WriteBatch) -> Result<(), OntolithError>;

    fn commit_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    fn abort_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError>;

    fn delete_by_key(&self, key: &StorageKey) -> Result<usize, OntolithError>;

    fn snapshot(&self) -> SnapshotRef;

    fn default_graph_triples(&self) -> Vec<Triple>;

    fn default_graph_triples_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn triples_by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn named_graph_quads(&self) -> Vec<Quad>;
}

pub trait TripleRepository: Send + Sync {
    fn insert(&self, txn_id: TxnId, triple: Triple) -> Result<(), OntolithError>;

    fn all_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple>;

    fn all(&self) -> Vec<Triple> {
        self.all_in_txn(None)
    }

    fn by_subject(&self, subject: NodeId) -> Vec<Triple> {
        self.by_subject_in_txn(subject, None)
    }
}

pub trait QuadRepository: Send + Sync {
    fn insert(&self, txn_id: TxnId, quad: Quad) -> Result<(), OntolithError>;

    fn all(&self) -> Vec<Quad>;

    fn by_graph_name(&self, graph_name: &Iri) -> Vec<Quad>;
}

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
