//! Storage domain types (L2).
//!
//! Write path units, WAL records, snapshot references, statistics, and
//! physical index encoding.

mod encoding;

pub use encoding::{
    IndexKind, encode_dictionary_entry, encode_osp_key, encode_osp_object_prefix, encode_pos_key,
    encode_pos_predicate_prefix, encode_spo_key, encode_spo_subject_prefix,
    encode_triple_index_key,
};

use ontolith_core::domain::{ConsistencyLevel, NodeId};
use ontolith_rdf::domain::{Quad, Triple};
use ontolith_transaction::domain::TxnId;

/// How secondary indexes are maintained relative to commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexMaintenance {
    /// Update indexes synchronously inside commit (default, correctness-first).
    #[default]
    Sync,
    /// Reserved: defer index work; not used by the memory engine yet.
    Async,
}

/// Logical key used by delete / index maintenance paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey {
    pub index: &'static str,
    pub components: Vec<NodeId>,
}

impl StorageKey {
    pub fn spo_subject(subject: NodeId) -> Self {
        Self {
            index: IndexKind::Spo.as_str(),
            components: vec![subject],
        }
    }

    pub fn index_kind(&self) -> Option<IndexKind> {
        match self.index {
            "spo" => Some(IndexKind::Spo),
            "pos" => Some(IndexKind::Pos),
            "osp" => Some(IndexKind::Osp),
            "sop" => Some(IndexKind::Sop),
            "pso" => Some(IndexKind::Pso),
            "ops" => Some(IndexKind::Ops),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteOperation {
    PutTriple(Triple),
    PutQuad(Quad),
    /// Exact triple delete from the default graph (idempotent if absent).
    DeleteTriple(Triple),
    /// Exact quad delete; `graph_name = None` targets the default graph.
    DeleteQuad(Quad),
    /// Prefix / key delete. For `index = "spo"` with one component, deletes
    /// all default-graph triples (and matching named-graph quads) for that subject.
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
    pub consistency: ConsistencyLevel,
}

/// Aggregate storage statistics for optimizers and observability (SAS-0001 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageStats {
    pub triple_count: u64,
    pub quad_count: u64,
    pub distinct_subjects: u64,
    pub distinct_predicates: u64,
    pub distinct_objects: u64,
    pub named_graph_count: u64,
    pub dictionary_entries: u64,
    pub pending_transactions: u64,
    pub wal_records: u64,
    pub index_kinds_active: u8,
}

pub fn status() -> &'static str {
    "domain"
}
