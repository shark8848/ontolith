use ontolith_core::domain::NodeId;
use ontolith_rdf::domain::{Quad, Triple};
use ontolith_transaction::domain::TxnId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey {
    pub index: &'static str,
    pub components: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteOperation {
    PutTriple(Triple),
    PutQuad(Quad),
    DeleteKey(StorageKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteBatch {
    pub txn_id: TxnId,
    pub operations: Vec<WriteOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalPhase {
    Staged,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WalRecord {
    pub txn_id: TxnId,
    pub phase: WalPhase,
    pub operation_count: usize,
    pub operations: Vec<WriteOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRef {
    pub snapshot_id: u64,
    pub read_txn_id: Option<TxnId>,
}

pub fn status() -> &'static str {
    "domain"
}
