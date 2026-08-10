use crate::application::{
    DictionaryCodec, QuadRepository, StorageEngine, TripleRepository, WriteAheadLog,
};
use crate::domain::{
    SnapshotRef, StorageKey, StorageStats, WalPhase, WalRecord, WriteBatch, WriteOperation,
};
use ontolith_core::domain::{ConsistencyLevel, Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_transaction::domain::TxnId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[cfg(feature = "rocksdb-backend")]
mod codec;
#[cfg(feature = "rocksdb-backend")]
mod indexes;

#[cfg(feature = "rocksdb-backend")]
mod rocks;

#[cfg(feature = "rocksdb-backend")]
pub use codec::{decode_term, encode_term};
#[cfg(feature = "rocksdb-backend")]
pub use rocks::{RaftCfOp, RocksDbStorageEngine, open_rocksdb_engine};
#[cfg(feature = "rocksdb-backend")]
pub use rocks::{SemanticCfEntry, SemanticCfOp};
#[cfg(feature = "rocksdb-backend")]
pub use rocks::{TenantCfEntry, TenantCfOp};

#[derive(Default)]
struct DictionaryState {
    next_node_id: u64,
    node_to_value: HashMap<NodeId, String>,
    value_to_node: HashMap<String, NodeId>,
}

pub struct InMemoryDictionary {
    state: RwLock<DictionaryState>,
}

impl InMemoryDictionary {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(DictionaryState::default()),
        }
    }
}

impl Default for InMemoryDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl DictionaryCodec for InMemoryDictionary {
    fn encode_node(&self, value: &str) -> NodeId {
        let mut guard = self
            .state
            .write()
            .expect("dictionary lock must not be poisoned");

        if let Some(existing) = guard.value_to_node.get(value) {
            return *existing;
        }

        guard.next_node_id += 1;
        let node_id = NodeId::new(guard.next_node_id);
        guard.node_to_value.insert(node_id, value.to_owned());
        guard.value_to_node.insert(value.to_owned(), node_id);
        node_id
    }

    fn decode_node(&self, node_id: NodeId) -> Option<String> {
        let guard = self.state.read().ok()?;
        guard.node_to_value.get(&node_id).cloned()
    }

    /// Non-mutating membership probe (overrides the trait default, which
    /// would insert unknown values via [`Self::encode_node`]).
    fn contains_value(&self, value: &str) -> bool {
        self.state
            .read()
            .map(|guard| guard.value_to_node.contains_key(value))
            .unwrap_or(false)
    }
}

/// Immutable committed graph at one version of the MVCC chain.
#[derive(Default)]
struct CommittedGraph {
    default_graph: Vec<Triple>,
    named_graph_quads: Vec<Quad>,
}

/// In-memory MVCC state: a commit-versioned chain of immutable graphs plus
/// staged (uncommitted) writes.
struct StorageState {
    /// Version chain keyed by commit sequence; `0` = genesis (empty) version.
    /// Pruning removes leading entries; `triples_at_version` clamps to the
    /// oldest retained version when the requested version was pruned.
    versions: BTreeMap<u64, Arc<CommittedGraph>>,
    /// Next commit sequence to assign (monotonic, includes pruned versions).
    next_version: u64,
    spo_index: HashMap<NodeId, Vec<Triple>>,
    pending_writes: HashMap<TxnId, Vec<WriteOperation>>,
}

impl Default for StorageState {
    fn default() -> Self {
        Self {
            versions: BTreeMap::from([(0, Arc::new(CommittedGraph::default()))]),
            next_version: 1,
            spo_index: HashMap::new(),
            pending_writes: HashMap::new(),
        }
    }
}

pub struct InMemoryStorageEngine {
    state: RwLock<StorageState>,
    next_snapshot_id: AtomicU64,
    staged_batches_count: AtomicU64,
    failed_stage_batches_count: AtomicU64,
    committed_txn_count: AtomicU64,
    failed_commit_txn_count: AtomicU64,
    committed_put_triple_ops_count: AtomicU64,
    committed_put_quad_ops_count: AtomicU64,
    committed_delete_key_ops_count: AtomicU64,
    aborted_txn_count: AtomicU64,
    failed_abort_txn_count: AtomicU64,
    aborted_put_triple_ops_count: AtomicU64,
    aborted_put_quad_ops_count: AtomicU64,
    aborted_delete_key_ops_count: AtomicU64,
    checkpoint_truncated_count: AtomicU64,
    pruned_versions_count: AtomicU64,
    /// Outstanding snapshots: snapshot id → pinned committed version.
    pinned_snapshots: Mutex<HashMap<u64, u64>>,
    /// Auto-prune retention applied after each commit/delete (default 16).
    version_retention: usize,
    wal: Arc<dyn WriteAheadLog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageMetricsSnapshot {
    pub staged_batches: u64,
    pub failed_stage_batches: u64,
    pub committed_transactions: u64,
    pub failed_commit_transactions: u64,
    pub committed_put_triple_operations: u64,
    pub committed_put_quad_operations: u64,
    pub committed_delete_key_operations: u64,
    pub aborted_transactions: u64,
    pub failed_abort_transactions: u64,
    pub aborted_put_triple_operations: u64,
    pub aborted_put_quad_operations: u64,
    pub aborted_delete_key_operations: u64,
    pub checkpoint_truncated_records: u64,
    pub pending_transactions: usize,
    pub wal_records: usize,
    pub committed_versions: u64,
    pub pruned_versions: u64,
    pub pinned_snapshots: u64,
}

impl InMemoryStorageEngine {
    pub fn new() -> Self {
        Self::with_version_retention(16)
    }

    pub fn with_wal(wal: Arc<dyn WriteAheadLog>) -> Self {
        Self::with_wal_and_retention(wal, 16)
    }

    pub fn with_version_retention(retention: usize) -> Self {
        Self::with_wal_and_retention(Arc::new(InMemoryWal::new()), retention)
    }

    fn with_wal_and_retention(wal: Arc<dyn WriteAheadLog>, version_retention: usize) -> Self {
        Self {
            state: RwLock::new(StorageState::default()),
            next_snapshot_id: AtomicU64::new(1),
            staged_batches_count: AtomicU64::new(0),
            failed_stage_batches_count: AtomicU64::new(0),
            committed_txn_count: AtomicU64::new(0),
            failed_commit_txn_count: AtomicU64::new(0),
            committed_put_triple_ops_count: AtomicU64::new(0),
            committed_put_quad_ops_count: AtomicU64::new(0),
            committed_delete_key_ops_count: AtomicU64::new(0),
            aborted_txn_count: AtomicU64::new(0),
            failed_abort_txn_count: AtomicU64::new(0),
            aborted_put_triple_ops_count: AtomicU64::new(0),
            aborted_put_quad_ops_count: AtomicU64::new(0),
            aborted_delete_key_ops_count: AtomicU64::new(0),
            checkpoint_truncated_count: AtomicU64::new(0),
            pruned_versions_count: AtomicU64::new(0),
            pinned_snapshots: Mutex::new(HashMap::new()),
            version_retention,
            wal,
        }
    }

    pub fn wal_entries(&self) -> Vec<WalRecord> {
        self.wal.entries()
    }

    pub fn checkpoint_wal(&self) -> Result<usize, OntolithError> {
        self.checkpoint_wal_with_retention(0)
    }

    pub fn checkpoint_wal_with_retention(
        &self,
        min_tail_records: usize,
    ) -> Result<usize, OntolithError> {
        let records = self.wal.entries();
        let mut open_txns = HashSet::new();
        let mut safe_upto = 0usize;

        for (idx, record) in records.iter().enumerate() {
            match record.phase {
                WalPhase::Staged => {
                    open_txns.insert(record.txn_id);
                }
                WalPhase::Committed | WalPhase::Aborted => {
                    open_txns.remove(&record.txn_id);
                }
            }

            if open_txns.is_empty() {
                safe_upto = idx + 1;
            }
        }

        let truncate_upto = safe_upto.saturating_sub(min_tail_records);

        if truncate_upto > 0 {
            self.wal.truncate_prefix(truncate_upto)?;
            self.checkpoint_truncated_count
                .fetch_add(truncate_upto as u64, Ordering::SeqCst);
        }

        Ok(truncate_upto)
    }

    pub fn metrics_snapshot(&self) -> StorageMetricsSnapshot {
        let (pending_transactions, committed_versions) = self
            .state
            .read()
            .map(|state| {
                (
                    state.pending_writes.len(),
                    state.next_version.saturating_sub(1),
                )
            })
            .unwrap_or((0, 0));
        let pinned_snapshots = self
            .pinned_snapshots
            .lock()
            .map(|pins| pins.len() as u64)
            .unwrap_or(0);

        StorageMetricsSnapshot {
            staged_batches: self.staged_batches_count.load(Ordering::SeqCst),
            failed_stage_batches: self.failed_stage_batches_count.load(Ordering::SeqCst),
            committed_transactions: self.committed_txn_count.load(Ordering::SeqCst),
            failed_commit_transactions: self.failed_commit_txn_count.load(Ordering::SeqCst),
            committed_put_triple_operations: self
                .committed_put_triple_ops_count
                .load(Ordering::SeqCst),
            committed_put_quad_operations: self.committed_put_quad_ops_count.load(Ordering::SeqCst),
            committed_delete_key_operations: self
                .committed_delete_key_ops_count
                .load(Ordering::SeqCst),
            aborted_transactions: self.aborted_txn_count.load(Ordering::SeqCst),
            failed_abort_transactions: self.failed_abort_txn_count.load(Ordering::SeqCst),
            aborted_put_triple_operations: self.aborted_put_triple_ops_count.load(Ordering::SeqCst),
            aborted_put_quad_operations: self.aborted_put_quad_ops_count.load(Ordering::SeqCst),
            aborted_delete_key_operations: self.aborted_delete_key_ops_count.load(Ordering::SeqCst),
            checkpoint_truncated_records: self.checkpoint_truncated_count.load(Ordering::SeqCst),
            pending_transactions,
            wal_records: self.wal.entries().len(),
            committed_versions,
            pruned_versions: self.pruned_versions_count.load(Ordering::SeqCst),
            pinned_snapshots,
        }
    }

    pub fn recover_from_wal(records: &[WalRecord]) -> Result<Self, OntolithError> {
        Self::recover_internal(records, false)
    }

    pub fn recover_from_wal_tolerant(records: &[WalRecord]) -> Result<Self, OntolithError> {
        Self::recover_internal(records, true)
    }

    fn recover_internal(records: &[WalRecord], tolerant: bool) -> Result<Self, OntolithError> {
        let wal = Arc::new(InMemoryWal::new());
        let mut state = StorageState::default();

        for record in records {
            wal.append(record.clone())?;
        }

        for record in records {
            match record.phase {
                WalPhase::Staged => {
                    state
                        .pending_writes
                        .entry(record.txn_id)
                        .or_default()
                        .extend(record.operations.clone());
                }
                WalPhase::Committed => {
                    let Some(operations) = state.pending_writes.remove(&record.txn_id) else {
                        if tolerant {
                            continue;
                        }
                        return Err(OntolithError::InvalidState(
                            "wal replay failed: committed transaction without staged operations",
                        ));
                    };

                    Self::commit_into_state(&mut state, operations);
                }
                WalPhase::Aborted => {
                    let removed = state.pending_writes.remove(&record.txn_id);
                    if removed.is_none() && !tolerant {
                        return Err(OntolithError::InvalidState(
                            "wal replay failed: aborted transaction without staged operations",
                        ));
                    }
                }
            }
        }

        Self::rebuild_spo_index(&mut state);

        Ok(Self {
            state: RwLock::new(state),
            next_snapshot_id: AtomicU64::new(1),
            staged_batches_count: AtomicU64::new(0),
            failed_stage_batches_count: AtomicU64::new(0),
            committed_txn_count: AtomicU64::new(0),
            failed_commit_txn_count: AtomicU64::new(0),
            committed_put_triple_ops_count: AtomicU64::new(0),
            committed_put_quad_ops_count: AtomicU64::new(0),
            committed_delete_key_ops_count: AtomicU64::new(0),
            aborted_txn_count: AtomicU64::new(0),
            failed_abort_txn_count: AtomicU64::new(0),
            aborted_put_triple_ops_count: AtomicU64::new(0),
            aborted_put_quad_ops_count: AtomicU64::new(0),
            aborted_delete_key_ops_count: AtomicU64::new(0),
            checkpoint_truncated_count: AtomicU64::new(0),
            pruned_versions_count: AtomicU64::new(0),
            pinned_snapshots: Mutex::new(HashMap::new()),
            version_retention: 16,
            wal,
        })
    }

    fn latest_committed(state: &StorageState) -> &CommittedGraph {
        state
            .versions
            .last_key_value()
            .map(|(_, graph)| graph.as_ref())
            .expect("version chain always contains at least the genesis version")
    }

    /// Apply a committed write batch as a new immutable version of the chain.
    fn commit_into_state(state: &mut StorageState, operations: Vec<WriteOperation>) {
        let latest = Self::latest_committed(state);
        let mut triples = latest.default_graph.clone();
        let mut quads = latest.named_graph_quads.clone();
        for op in operations {
            Self::apply_committed_op(&mut triples, &mut quads, op);
        }
        let seq = state.next_version;
        state.next_version += 1;
        state.versions.insert(
            seq,
            Arc::new(CommittedGraph {
                default_graph: triples,
                named_graph_quads: quads,
            }),
        );
    }

    fn remove_by_subject_vecs(
        triples: &mut Vec<Triple>,
        quads: &mut Vec<Quad>,
        subject_id: NodeId,
    ) -> usize {
        let before_default = triples.len();
        triples.retain(|t| t.subject != subject_id);
        let removed_default = before_default - triples.len();

        let before_quads = quads.len();
        quads.retain(|q| q.triple.subject != subject_id);
        let removed_quads = before_quads - quads.len();

        removed_default + removed_quads
    }

    fn rebuild_spo_index(state: &mut StorageState) {
        state.spo_index.clear();
        let triples = Self::latest_committed(state).default_graph.clone();
        for triple in &triples {
            state
                .spo_index
                .entry(triple.subject)
                .or_default()
                .push(triple.clone());
        }
    }

    fn apply_ops_to_triple_projection(
        triples: &mut Vec<Triple>,
        operations: &[WriteOperation],
        subject_filter: Option<NodeId>,
        predicate_filter: Option<&Iri>,
        object_filter: Option<&Term>,
    ) {
        for op in operations {
            match op {
                WriteOperation::PutTriple(triple) => {
                    let subject_ok = subject_filter.is_none_or(|subject| subject == triple.subject);
                    let predicate_ok =
                        predicate_filter.is_none_or(|predicate| predicate == &triple.predicate);
                    let object_ok = object_filter.is_none_or(|object| object == &triple.object);
                    if subject_ok
                        && predicate_ok
                        && object_ok
                        && !triples.iter().any(|t| t == triple)
                    {
                        triples.push(triple.clone());
                    }
                }
                WriteOperation::DeleteTriple(triple) => {
                    triples.retain(|existing| existing != triple);
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied()
                        && subject_filter.is_none_or(|subject| subject == subject_id)
                    {
                        triples.retain(|existing| existing.subject != subject_id);
                    }
                }
                WriteOperation::PutQuad(_) | WriteOperation::DeleteQuad(_) => {}
            }
        }
    }

    fn apply_ops_to_quad_projection(
        quads: &mut Vec<Quad>,
        operations: &[WriteOperation],
        graph_filter: Option<&Iri>,
    ) {
        for op in operations {
            match op {
                WriteOperation::PutQuad(quad) => {
                    let graph_ok = graph_filter.is_none_or(|g| quad.graph_name.as_ref() == Some(g));
                    if graph_ok && !quads.iter().any(|existing| existing == quad) {
                        quads.push(quad.clone());
                    }
                }
                WriteOperation::DeleteQuad(quad) => {
                    let graph_ok = graph_filter.is_none_or(|g| quad.graph_name.as_ref() == Some(g));
                    if graph_ok {
                        quads.retain(|existing| existing != quad);
                    }
                }
                WriteOperation::PutTriple(triple) => {
                    if graph_filter.is_none()
                        && !quads.iter().any(|existing| existing.triple == *triple)
                    {
                        quads.push(Quad::in_default_graph(triple.clone()));
                    }
                }
                WriteOperation::DeleteTriple(triple) => {
                    if graph_filter.is_none() {
                        quads.retain(|existing| existing.triple != *triple);
                    }
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied() {
                        quads.retain(|existing| existing.triple.subject != subject_id);
                    }
                }
            }
        }
    }

    fn apply_committed_op(triples: &mut Vec<Triple>, quads: &mut Vec<Quad>, op: WriteOperation) {
        match op {
            WriteOperation::PutTriple(triple) => {
                if !triples.iter().any(|t| t == &triple) {
                    triples.push(triple);
                }
            }
            WriteOperation::PutQuad(quad) => {
                if !quads.iter().any(|q| q == &quad) {
                    quads.push(quad);
                }
            }
            WriteOperation::DeleteTriple(triple) => {
                triples.retain(|t| t != &triple);
            }
            WriteOperation::DeleteQuad(quad) => {
                quads.retain(|q| q != &quad);
            }
            WriteOperation::DeleteKey(key) => {
                if let Some(subject_id) = key.components.first().copied() {
                    let _ = Self::remove_by_subject_vecs(triples, quads, subject_id);
                }
            }
        }
    }

    /// Prune leading versions beyond `retention` (genesis always kept); a
    /// version pinned by an outstanding snapshot is never pruned, and the
    /// newest committed version is always retained.
    fn prune_locked(&self, state: &mut StorageState, retention: usize) -> usize {
        let keep = retention.max(1);
        let pinned: HashSet<u64> = self
            .pinned_snapshots
            .lock()
            .map(|pins| pins.values().copied().collect())
            .unwrap_or_default();
        let latest_seq = state.next_version.saturating_sub(1);
        let mut pruned = 0usize;
        let seqs: Vec<u64> = state.versions.keys().copied().collect();
        for seq in seqs {
            if seq == 0 || seq == latest_seq || pinned.contains(&seq) {
                continue;
            }
            let retained_committed = state.versions.len().saturating_sub(1);
            if retained_committed <= keep {
                break;
            }
            state.versions.remove(&seq);
            pruned += 1;
        }
        if pruned > 0 {
            self.pruned_versions_count
                .fetch_add(pruned as u64, Ordering::SeqCst);
        }
        pruned
    }
}

impl Default for InMemoryStorageEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageEngine for InMemoryStorageEngine {
    fn apply_write_batch(&self, batch: &WriteBatch) -> Result<(), OntolithError> {
        let mut guard = self.state.write().map_err(|_| {
            self.failed_stage_batches_count
                .fetch_add(1, Ordering::SeqCst);
            OntolithError::InvalidState("storage state lock poisoned")
        })?;

        guard
            .pending_writes
            .entry(batch.txn_id)
            .or_default()
            .extend(batch.operations.clone());

        if let Err(err) = self.wal.append(WalRecord {
            txn_id: batch.txn_id,
            phase: WalPhase::Staged,
            operation_count: batch.operations.len(),
            operations: batch.operations.clone(),
        }) {
            self.failed_stage_batches_count
                .fetch_add(1, Ordering::SeqCst);
            return Err(err);
        }

        self.staged_batches_count.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    fn commit_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        let mut guard = self.state.write().map_err(|_| {
            self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
            OntolithError::InvalidState("storage state lock poisoned")
        })?;

        let Some(operations) = guard.pending_writes.remove(&txn_id) else {
            self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
            return Err(OntolithError::InvalidState(
                "pending storage transaction not found",
            ));
        };

        let mut put_triple_ops = 0u64;
        let mut put_quad_ops = 0u64;
        let mut delete_key_ops = 0u64;

        for op in &operations {
            match op {
                WriteOperation::PutTriple(_) => put_triple_ops += 1,
                WriteOperation::PutQuad(_) => put_quad_ops += 1,
                WriteOperation::DeleteKey(_)
                | WriteOperation::DeleteTriple(_)
                | WriteOperation::DeleteQuad(_) => delete_key_ops += 1,
            }
        }
        Self::commit_into_state(&mut guard, operations);
        Self::rebuild_spo_index(&mut guard);
        self.prune_locked(&mut guard, self.version_retention);

        if let Err(err) = self.wal.append(WalRecord {
            txn_id,
            phase: WalPhase::Committed,
            operation_count: 0,
            operations: Vec::new(),
        }) {
            self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
            return Err(err);
        }

        self.committed_txn_count.fetch_add(1, Ordering::SeqCst);
        self.committed_put_triple_ops_count
            .fetch_add(put_triple_ops, Ordering::SeqCst);
        self.committed_put_quad_ops_count
            .fetch_add(put_quad_ops, Ordering::SeqCst);
        self.committed_delete_key_ops_count
            .fetch_add(delete_key_ops, Ordering::SeqCst);

        Ok(())
    }

    fn abort_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        let mut guard = self.state.write().map_err(|_| {
            self.failed_abort_txn_count.fetch_add(1, Ordering::SeqCst);
            OntolithError::InvalidState("storage state lock poisoned")
        })?;
        let removed = guard.pending_writes.remove(&txn_id);

        if let Some(ops) = removed {
            let mut put_triple_ops = 0u64;
            let mut put_quad_ops = 0u64;
            let mut delete_key_ops = 0u64;

            for op in &ops {
                match op {
                    WriteOperation::PutTriple(_) => put_triple_ops += 1,
                    WriteOperation::PutQuad(_) => put_quad_ops += 1,
                    WriteOperation::DeleteKey(_)
                    | WriteOperation::DeleteTriple(_)
                    | WriteOperation::DeleteQuad(_) => delete_key_ops += 1,
                }
            }

            if let Err(err) = self.wal.append(WalRecord {
                txn_id,
                phase: WalPhase::Aborted,
                operation_count: ops.len(),
                operations: Vec::new(),
            }) {
                self.failed_abort_txn_count.fetch_add(1, Ordering::SeqCst);
                return Err(err);
            }

            self.aborted_txn_count.fetch_add(1, Ordering::SeqCst);
            self.aborted_put_triple_ops_count
                .fetch_add(put_triple_ops, Ordering::SeqCst);
            self.aborted_put_quad_ops_count
                .fetch_add(put_quad_ops, Ordering::SeqCst);
            self.aborted_delete_key_ops_count
                .fetch_add(delete_key_ops, Ordering::SeqCst);
        }

        Ok(())
    }

    fn delete_by_key(&self, key: &StorageKey) -> Result<usize, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("storage state lock poisoned"))?;

        let Some(subject_id) = key.components.first().copied() else {
            return Ok(0);
        };

        let latest = Self::latest_committed(&guard);
        let mut triples = latest.default_graph.clone();
        let mut quads = latest.named_graph_quads.clone();
        let removed = Self::remove_by_subject_vecs(&mut triples, &mut quads, subject_id);
        if removed > 0 {
            let seq = guard.next_version;
            guard.next_version += 1;
            guard.versions.insert(
                seq,
                Arc::new(CommittedGraph {
                    default_graph: triples,
                    named_graph_quads: quads,
                }),
            );
            Self::rebuild_spo_index(&mut guard);
            self.prune_locked(&mut guard, self.version_retention);
        }
        Ok(removed)
    }

    fn snapshot(&self) -> SnapshotRef {
        self.snapshot_with(ConsistencyLevel::Strong, None)
    }

    fn snapshot_with(
        &self,
        consistency: ConsistencyLevel,
        read_txn_id: Option<TxnId>,
    ) -> SnapshotRef {
        let version = self
            .state
            .read()
            .map(|state| state.next_version.saturating_sub(1))
            .unwrap_or(0);
        let snapshot_id = self.next_snapshot_id.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut pins) = self.pinned_snapshots.lock() {
            pins.insert(snapshot_id, version);
        }
        SnapshotRef::new(snapshot_id, read_txn_id, consistency, version)
    }

    fn stats(&self) -> StorageStats {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return StorageStats::default(),
        };

        let mut subjects = HashSet::new();
        let mut predicates = HashSet::new();
        let mut objects: Vec<Term> = Vec::new();
        let mut named_graphs = HashSet::new();

        let latest = Self::latest_committed(&guard);
        for triple in &latest.default_graph {
            subjects.insert(triple.subject);
            predicates.insert(triple.predicate.clone());
            if !objects.iter().any(|o| o == &triple.object) {
                objects.push(triple.object.clone());
            }
        }

        for quad in &latest.named_graph_quads {
            if let Some(g) = &quad.graph_name {
                named_graphs.insert(g.clone());
            }
        }

        StorageStats {
            triple_count: latest.default_graph.len() as u64,
            quad_count: latest.named_graph_quads.len() as u64,
            distinct_subjects: subjects.len() as u64,
            distinct_predicates: predicates.len() as u64,
            distinct_objects: objects.len() as u64,
            named_graph_count: named_graphs.len() as u64,
            dictionary_entries: 0,
            pending_transactions: guard.pending_writes.len() as u64,
            wal_records: self.wal.entries().len() as u64,
            index_kinds_active: 1,
            committed_versions: guard.next_version.saturating_sub(1),
            pruned_versions: self.pruned_versions_count.load(Ordering::SeqCst),
            pinned_snapshots: self
                .pinned_snapshots
                .lock()
                .map(|pins| pins.len() as u64)
                .unwrap_or(0),
        }
    }

    fn default_graph_triples(&self) -> Vec<Triple> {
        self.default_graph_triples_in_txn(None)
    }

    fn default_graph_triples_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };

        let mut triples = Self::latest_committed(&guard).default_graph.clone();
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, operations, None, None, None);
        }
        triples
    }

    fn triples_by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };

        let mut triples = guard.spo_index.get(&subject).cloned().unwrap_or_default();
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(
                &mut triples,
                operations,
                Some(subject),
                None,
                None,
            );
        }
        triples
    }

    fn triples_by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };

        let mut triples: Vec<Triple> = Self::latest_committed(&guard)
            .default_graph
            .iter()
            .filter(|t| &t.predicate == predicate)
            .cloned()
            .collect();

        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(
                &mut triples,
                operations,
                None,
                Some(predicate),
                None,
            );
        }

        triples
    }

    fn triples_by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };

        let mut triples: Vec<Triple> = Self::latest_committed(&guard)
            .default_graph
            .iter()
            .filter(|t| &t.object == object)
            .cloned()
            .collect();

        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(
                &mut triples,
                operations,
                None,
                None,
                Some(object),
            );
        }

        triples
    }

    fn named_graph_quads(&self) -> Vec<Quad> {
        self.state
            .read()
            .map(|s| Self::latest_committed(&s).named_graph_quads.clone())
            .unwrap_or_default()
    }

    fn committed_version(&self) -> u64 {
        self.state
            .read()
            .map(|s| s.next_version.saturating_sub(1))
            .unwrap_or(0)
    }

    fn version_count(&self) -> u64 {
        self.state
            .read()
            .map(|s| s.next_version.saturating_sub(1))
            .unwrap_or(0)
    }

    fn pruned_version_count(&self) -> u64 {
        self.pruned_versions_count.load(Ordering::SeqCst)
    }

    fn pinned_snapshot_count(&self) -> u64 {
        self.pinned_snapshots
            .lock()
            .map(|pins| pins.len() as u64)
            .unwrap_or(0)
    }

    fn release_snapshot(&self, snapshot_id: u64) {
        if let Ok(mut pins) = self.pinned_snapshots.lock() {
            pins.remove(&snapshot_id);
        }
    }

    fn prune_versions(&self, retention: usize) -> Result<usize, OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("storage state lock poisoned"))?;
        Ok(self.prune_locked(&mut guard, retention))
    }

    fn triples_at_version_in_txn(&self, version: u64, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };
        let graph = guard
            .versions
            .get(&version)
            .or_else(|| guard.versions.first_key_value().map(|(_, g)| g))
            .map(|g| g.as_ref());
        let Some(graph) = graph else {
            return Vec::new();
        };
        let mut triples = graph.default_graph.clone();
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, operations, None, None, None);
        }
        triples
    }

    fn named_graph_quads_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Quad> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };
        let mut quads = Self::latest_committed(&guard).named_graph_quads.clone();
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_quad_projection(&mut quads, operations, None);
        }
        quads
    }

    fn quads_by_graph_in_txn(&self, graph_name: Option<&Iri>, txn_id: Option<TxnId>) -> Vec<Quad> {
        let guard = match self.state.read() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };
        let mut quads = match graph_name {
            Some(g) => Self::latest_committed(&guard)
                .named_graph_quads
                .iter()
                .filter(|q| q.graph_name.as_ref() == Some(g))
                .cloned()
                .collect(),
            None => Self::latest_committed(&guard)
                .default_graph
                .iter()
                .map(|t| Quad::in_default_graph(t.clone()))
                .collect(),
        };
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_quad_projection(&mut quads, operations, graph_name);
        }
        quads
    }

    fn quads_at_version(&self, version: u64) -> Vec<Quad> {
        self.state
            .read()
            .map(|s| {
                s.versions
                    .get(&version)
                    .or_else(|| s.versions.first_key_value().map(|(_, g)| g))
                    .map(|g| g.named_graph_quads.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

pub struct InMemoryWal {
    records: RwLock<Vec<WalRecord>>,
}

impl InMemoryWal {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryWal {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteAheadLog for InMemoryWal {
    fn append(&self, record: WalRecord) -> Result<(), OntolithError> {
        let mut guard = self
            .records
            .write()
            .map_err(|_| OntolithError::InvalidState("wal lock poisoned"))?;
        guard.push(record);
        Ok(())
    }

    fn entries(&self) -> Vec<WalRecord> {
        self.records
            .read()
            .map(|records| records.clone())
            .unwrap_or_default()
    }

    fn truncate_prefix(&self, upto_exclusive: usize) -> Result<(), OntolithError> {
        let mut guard = self
            .records
            .write()
            .map_err(|_| OntolithError::InvalidState("wal lock poisoned"))?;

        if upto_exclusive == 0 {
            return Ok(());
        }

        if upto_exclusive >= guard.len() {
            guard.clear();
            return Ok(());
        }

        guard.drain(0..upto_exclusive);
        Ok(())
    }
}

pub struct InMemoryTripleRepository {
    engine: Arc<InMemoryStorageEngine>,
}

impl InMemoryTripleRepository {
    pub fn new(engine: Arc<InMemoryStorageEngine>) -> Self {
        Self { engine }
    }
}

impl TripleRepository for InMemoryTripleRepository {
    fn insert(&self, txn_id: TxnId, triple: Triple) -> Result<(), OntolithError> {
        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutTriple(triple)],
        };
        self.engine.apply_write_batch(&batch)
    }

    fn all_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.default_graph_triples_in_txn(txn_id)
    }

    fn all_at_version_in_txn(&self, version: u64, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_at_version_in_txn(version, txn_id)
    }

    fn by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_subject_in_txn(subject, txn_id)
    }

    fn by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_predicate_in_txn(predicate, txn_id)
    }

    fn by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_object_in_txn(object, txn_id)
    }
}

/// StorageEngine-backed triple repository adapter usable across memory/rocksdb engines.
pub struct EngineTripleRepository {
    engine: Arc<dyn StorageEngine>,
}

impl EngineTripleRepository {
    pub fn new(engine: Arc<dyn StorageEngine>) -> Self {
        Self { engine }
    }
}

impl TripleRepository for EngineTripleRepository {
    fn insert(&self, txn_id: TxnId, triple: Triple) -> Result<(), OntolithError> {
        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutTriple(triple)],
        };
        self.engine.apply_write_batch(&batch)
    }

    fn all_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.default_graph_triples_in_txn(txn_id)
    }

    fn all_at_version_in_txn(&self, version: u64, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_at_version_in_txn(version, txn_id)
    }

    fn by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_subject_in_txn(subject, txn_id)
    }

    fn by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_predicate_in_txn(predicate, txn_id)
    }

    fn by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_object_in_txn(object, txn_id)
    }
}

pub struct InMemoryQuadRepository {
    engine: Arc<InMemoryStorageEngine>,
}

impl InMemoryQuadRepository {
    pub fn new(engine: Arc<InMemoryStorageEngine>) -> Self {
        Self { engine }
    }
}

impl QuadRepository for InMemoryQuadRepository {
    fn insert(&self, txn_id: TxnId, quad: Quad) -> Result<(), OntolithError> {
        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutQuad(quad)],
        };
        self.engine.apply_write_batch(&batch)
    }

    fn all(&self) -> Vec<Quad> {
        self.engine.named_graph_quads()
    }

    fn by_graph_name(&self, graph_name: &Iri) -> Vec<Quad> {
        self.engine
            .named_graph_quads()
            .into_iter()
            .filter(|quad| quad.graph_name.as_ref() == Some(graph_name))
            .collect()
    }

    fn all_at_version(&self, version: u64) -> Vec<Quad> {
        self.engine.quads_at_version(version)
    }

    fn by_graph_name_at_version(&self, version: u64, graph_name: &Iri) -> Vec<Quad> {
        self.engine
            .quads_at_version(version)
            .into_iter()
            .filter(|quad| quad.graph_name.as_ref() == Some(graph_name))
            .collect()
    }
}

/// [`QuadRepository`] façade over any [`StorageEngine`] (memory or RocksDB).
pub struct EngineQuadRepository {
    engine: Arc<dyn StorageEngine>,
}

impl EngineQuadRepository {
    pub fn new(engine: Arc<dyn StorageEngine>) -> Self {
        Self { engine }
    }
}

impl QuadRepository for EngineQuadRepository {
    fn insert(&self, txn_id: TxnId, quad: Quad) -> Result<(), OntolithError> {
        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutQuad(quad)],
        };
        self.engine.apply_write_batch(&batch)
    }

    fn all(&self) -> Vec<Quad> {
        self.engine.named_graph_quads()
    }

    fn by_graph_name(&self, graph_name: &Iri) -> Vec<Quad> {
        self.engine.quads_by_graph_in_txn(Some(graph_name), None)
    }

    fn by_graph_name_in_txn(&self, graph_name: &Iri, txn_id: Option<TxnId>) -> Vec<Quad> {
        self.engine.quads_by_graph_in_txn(Some(graph_name), txn_id)
    }

    fn all_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Quad> {
        self.engine.named_graph_quads_in_txn(txn_id)
    }

    fn all_at_version(&self, version: u64) -> Vec<Quad> {
        self.engine.quads_at_version(version)
    }

    fn by_graph_name_at_version(&self, version: u64, graph_name: &Iri) -> Vec<Quad> {
        self.engine
            .quads_at_version(version)
            .into_iter()
            .filter(|quad| quad.graph_name.as_ref() == Some(graph_name))
            .collect()
    }
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::{
        EngineTripleRepository, InMemoryDictionary, InMemoryQuadRepository, InMemoryStorageEngine,
        InMemoryTripleRepository,
    };
    use crate::application::{
        DictionaryCodec, QuadRepository, StorageEngine, TransactionalWriteService,
        TripleRepository, WriteAheadLog,
    };
    use crate::domain::{StorageKey, WalPhase, WalRecord, WriteBatch, WriteOperation};
    use ontolith_core::domain::{ConsistencyLevel, Iri, NodeId};
    use ontolith_core::error::OntolithError;
    use ontolith_rdf::domain::{Quad, Term, Triple};
    use ontolith_transaction::domain::{TxnId, TxnMode};
    use ontolith_transaction::infrastructure::InMemoryTransactionManager;
    use std::sync::Arc;
    use std::sync::RwLock;

    struct FailOnPhaseWal {
        fail_phase: Option<WalPhase>,
        records: RwLock<Vec<WalRecord>>,
    }

    impl FailOnPhaseWal {
        fn new(fail_phase: Option<WalPhase>) -> Self {
            Self {
                fail_phase,
                records: RwLock::new(Vec::new()),
            }
        }
    }

    impl WriteAheadLog for FailOnPhaseWal {
        fn append(&self, record: WalRecord) -> Result<(), OntolithError> {
            if self.fail_phase.is_some_and(|phase| phase == record.phase) {
                return Err(OntolithError::InvalidState("injected wal append failure"));
            }

            let mut guard = self
                .records
                .write()
                .map_err(|_| OntolithError::InvalidState("wal lock poisoned"))?;
            guard.push(record);
            Ok(())
        }

        fn entries(&self) -> Vec<WalRecord> {
            self.records
                .read()
                .map(|records| records.clone())
                .unwrap_or_default()
        }

        fn truncate_prefix(&self, upto_exclusive: usize) -> Result<(), OntolithError> {
            let mut guard = self
                .records
                .write()
                .map_err(|_| OntolithError::InvalidState("wal lock poisoned"))?;

            if upto_exclusive == 0 {
                return Ok(());
            }

            if upto_exclusive >= guard.len() {
                guard.clear();
                return Ok(());
            }

            guard.drain(0..upto_exclusive);
            Ok(())
        }
    }

    #[test]
    fn dictionary_roundtrip_keeps_same_node_id() {
        let dictionary = InMemoryDictionary::new();
        let id_a = dictionary.encode_node("urn:test:alice");
        let id_b = dictionary.encode_node("urn:test:alice");

        assert_eq!(id_a, id_b);
        assert_eq!(
            dictionary.decode_node(id_a).as_deref(),
            Some("urn:test:alice")
        );
    }

    #[test]
    fn storage_applies_batch_and_supports_delete() {
        let storage = InMemoryStorageEngine::new();
        let txn_id = TxnId::new(1);
        let triple = Triple {
            subject: NodeId::new(10),
            predicate: Iri::new("urn:test:knows"),
            object: Term::Iri(Iri::new("urn:test:bob")),
        };

        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutTriple(triple.clone())],
        };

        storage
            .apply_write_batch(&batch)
            .expect("write batch must succeed");
        storage
            .commit_transaction(txn_id)
            .expect("storage commit must succeed");
        assert_eq!(storage.default_graph_triples().len(), 1);

        let removed = storage
            .delete_by_key(&StorageKey {
                index: "S",
                components: vec![triple.subject],
            })
            .expect("delete must succeed");

        assert_eq!(removed, 1);
        assert!(storage.default_graph_triples().is_empty());
    }

    #[test]
    fn storage_abort_discards_pending_writes() {
        let storage = InMemoryStorageEngine::new();
        let txn_id = TxnId::new(11);

        let batch = WriteBatch {
            txn_id,
            operations: vec![WriteOperation::PutTriple(Triple {
                subject: NodeId::new(77),
                predicate: Iri::new("urn:test:temp"),
                object: Term::Iri(Iri::new("urn:test:object")),
            })],
        };

        storage.apply_write_batch(&batch).expect("write must stage");
        storage
            .abort_transaction(txn_id)
            .expect("abort must discard writes");

        assert!(storage.default_graph_triples().is_empty());
    }

    #[test]
    fn pending_writes_visible_only_within_same_transaction() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = InMemoryTripleRepository::new(Arc::clone(&engine));
        let txn_id = TxnId::new(20);
        let subject = NodeId::new(222);

        repo.insert(
            txn_id,
            Triple {
                subject,
                predicate: Iri::new("urn:test:pending"),
                object: Term::Iri(Iri::new("urn:test:value")),
            },
        )
        .expect("insert must stage");

        assert!(repo.all().is_empty());
        assert_eq!(repo.all_in_txn(Some(txn_id)).len(), 1);
        assert_eq!(repo.by_subject_in_txn(subject, Some(txn_id)).len(), 1);
        assert!(
            repo.by_subject_in_txn(subject, Some(TxnId::new(999)))
                .is_empty()
        );

        engine
            .commit_transaction(txn_id)
            .expect("commit must make data globally visible");
        assert_eq!(repo.all().len(), 1);
    }

    #[test]
    fn snapshot_ids_increase_monotonically() {
        let storage = InMemoryStorageEngine::new();

        let snap1 = storage.snapshot();
        let snap2 = storage.snapshot();

        assert!(snap2.snapshot_id > snap1.snapshot_id);
    }

    #[test]
    fn triple_repository_supports_insert_and_subject_lookup() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = InMemoryTripleRepository::new(Arc::clone(&engine));
        let subject = NodeId::new(42);

        repo.insert(
            TxnId::new(9),
            Triple {
                subject,
                predicate: Iri::new("urn:test:likes"),
                object: Term::Iri(Iri::new("urn:test:rdf")),
            },
        )
        .expect("insert must succeed");
        engine
            .commit_transaction(TxnId::new(9))
            .expect("commit must make data visible");

        assert_eq!(repo.all().len(), 1);
        assert_eq!(repo.by_subject(subject).len(), 1);
        assert!(repo.by_subject(NodeId::new(99)).is_empty());
    }

    #[test]
    fn quad_repository_filters_by_graph_name() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = InMemoryQuadRepository::new(Arc::clone(&engine));
        let graph = Iri::new("urn:graph:main");

        repo.insert(
            TxnId::new(10),
            Quad {
                triple: Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:p"),
                    object: Term::Iri(Iri::new("urn:test:o")),
                },
                graph_name: Some(graph.clone()),
            },
        )
        .expect("quad insert must succeed");
        engine
            .commit_transaction(TxnId::new(10))
            .expect("commit must make data visible");

        assert_eq!(repo.all().len(), 1);
        assert_eq!(repo.by_graph_name(&graph).len(), 1);
        assert!(repo.by_graph_name(&Iri::new("urn:graph:other")).is_empty());
    }

    #[test]
    fn storage_matches_named_graph_quads_by_bound_positions() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let txn = TxnId::new(11);
        let graph = Iri::new("urn:graph:main");
        for (s, p, o) in [
            (1u64, "urn:p", "urn:o1"),
            (1u64, "urn:q", "urn:o2"),
            (2u64, "urn:p", "urn:o3"),
        ] {
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutQuad(Quad::in_named_graph(
                        Triple::new(NodeId::new(s), Iri::new(p), Term::Iri(Iri::new(o))),
                        graph.clone(),
                    ))],
                })
                .expect("stage");
        }
        engine.commit_transaction(txn).expect("commit");

        let all = engine.quads_matching_in_graph(&graph, None, None, None, None);
        assert_eq!(all.len(), 3);
        let by_subject =
            engine.quads_matching_in_graph(&graph, Some(NodeId::new(1)), None, None, None);
        assert_eq!(by_subject.len(), 2);
        let exact = engine.quads_matching_in_graph(
            &graph,
            Some(NodeId::new(2)),
            Some(&Iri::new("urn:p")),
            Some(&Term::Iri(Iri::new("urn:o3"))),
            None,
        );
        assert_eq!(exact.len(), 1);
        let other =
            engine.quads_matching_in_graph(&Iri::new("urn:graph:other"), None, None, None, None);
        assert!(other.is_empty());
    }

    #[test]
    fn transactional_write_service_commits_storage_and_transaction() {
        let tx_manager = InMemoryTransactionManager::new();
        let storage = InMemoryStorageEngine::new();
        let service = TransactionalWriteService::new(&tx_manager, &storage);

        service
            .commit_write_operations(
                TxnMode::ReadWrite,
                vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(555),
                    predicate: Iri::new("urn:test:managed"),
                    object: Term::Iri(Iri::new("urn:test:triple")),
                })],
            )
            .expect("transactional write must succeed");

        assert_eq!(storage.default_graph_triples().len(), 1);
    }

    #[test]
    fn wal_records_staged_and_committed_phases() {
        let storage = InMemoryStorageEngine::new();
        let txn_id = TxnId::new(333);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:wal"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("staging should succeed");

        storage
            .commit_transaction(txn_id)
            .expect("commit should succeed");

        let records = storage.wal_entries();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].phase, WalPhase::Staged);
        assert_eq!(records[0].operations.len(), 1);
        assert_eq!(records[1].phase, WalPhase::Committed);
        assert!(records[1].operations.is_empty());
    }

    #[test]
    fn recover_from_wal_restores_committed_state() {
        let storage = InMemoryStorageEngine::new();
        let committed_txn = TxnId::new(401);
        let aborted_txn = TxnId::new(402);
        let pending_txn = TxnId::new(403);

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: committed_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(77),
                    predicate: Iri::new("urn:test:replay:committed"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("committed batch stage should succeed");
        storage
            .commit_transaction(committed_txn)
            .expect("commit should succeed");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: aborted_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(88),
                    predicate: Iri::new("urn:test:replay:aborted"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("aborted batch stage should succeed");
        storage
            .abort_transaction(aborted_txn)
            .expect("abort should succeed");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: pending_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(99),
                    predicate: Iri::new("urn:test:replay:pending"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("pending batch stage should succeed");

        let replayed = InMemoryStorageEngine::recover_from_wal(&storage.wal_entries())
            .expect("wal replay should succeed");
        let repo = InMemoryTripleRepository::new(Arc::new(replayed));

        assert_eq!(repo.all().len(), 1);
        assert_eq!(repo.by_subject(NodeId::new(77)).len(), 1);
        assert!(repo.by_subject(NodeId::new(88)).is_empty());
        assert!(repo.by_subject(NodeId::new(99)).is_empty());
        assert_eq!(
            repo.by_subject_in_txn(NodeId::new(99), Some(pending_txn))
                .len(),
            1
        );
    }

    #[test]
    fn strict_recovery_rejects_committed_without_stage() {
        let records = vec![WalRecord {
            txn_id: TxnId::new(701),
            phase: WalPhase::Committed,
            operation_count: 0,
            operations: Vec::new(),
        }];

        match InMemoryStorageEngine::recover_from_wal(&records) {
            Ok(_) => panic!("strict recovery should reject malformed wal"),
            Err(err) => {
                assert_eq!(
                    err,
                    OntolithError::InvalidState(
                        "wal replay failed: committed transaction without staged operations"
                    )
                );
            }
        }
    }

    #[test]
    fn tolerant_recovery_ignores_malformed_tail_records() {
        let records = vec![
            WalRecord {
                txn_id: TxnId::new(801),
                phase: WalPhase::Staged,
                operation_count: 1,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(12),
                    predicate: Iri::new("urn:test:ok"),
                    object: Term::Iri(Iri::new("urn:test:ok:value")),
                })],
            },
            WalRecord {
                txn_id: TxnId::new(801),
                phase: WalPhase::Committed,
                operation_count: 0,
                operations: Vec::new(),
            },
            WalRecord {
                txn_id: TxnId::new(999),
                phase: WalPhase::Committed,
                operation_count: 0,
                operations: Vec::new(),
            },
        ];

        let recovered = InMemoryStorageEngine::recover_from_wal_tolerant(&records)
            .expect("tolerant recovery should skip malformed tail records");
        let repo = InMemoryTripleRepository::new(Arc::new(recovered));
        assert_eq!(repo.all().len(), 1);
        assert_eq!(repo.by_subject(NodeId::new(12)).len(), 1);
    }

    #[test]
    fn wal_checkpoint_truncates_closed_prefix_and_keeps_pending_tail() {
        let storage = InMemoryStorageEngine::new();
        let committed_txn = TxnId::new(901);
        let pending_txn = TxnId::new(902);

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: committed_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:checkpoint:committed"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage committed transaction");
        storage
            .commit_transaction(committed_txn)
            .expect("commit transaction");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: pending_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(2),
                    predicate: Iri::new("urn:test:checkpoint:pending"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage pending transaction");

        let removed = storage
            .checkpoint_wal()
            .expect("checkpoint should truncate closed prefix");
        let remaining = storage.wal_entries();

        assert_eq!(removed, 2);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].txn_id, pending_txn);
        assert_eq!(remaining[0].phase, WalPhase::Staged);
    }

    #[test]
    fn wal_checkpoint_can_clear_when_all_transactions_closed() {
        let storage = InMemoryStorageEngine::new();
        let committed_txn = TxnId::new(911);
        let aborted_txn = TxnId::new(912);

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: committed_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(11),
                    predicate: Iri::new("urn:test:checkpoint:commit"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage committed transaction");
        storage
            .commit_transaction(committed_txn)
            .expect("commit transaction");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: aborted_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(12),
                    predicate: Iri::new("urn:test:checkpoint:abort"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage aborted transaction");
        storage
            .abort_transaction(aborted_txn)
            .expect("abort transaction");

        let removed = storage
            .checkpoint_wal()
            .expect("checkpoint should clear closed WAL records");
        assert_eq!(removed, 4);
        assert!(storage.wal_entries().is_empty());
    }

    #[test]
    fn wal_checkpoint_with_retention_keeps_tail_records() {
        let storage = InMemoryStorageEngine::new();
        let committed_txn = TxnId::new(921);
        let aborted_txn = TxnId::new(922);

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: committed_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(21),
                    predicate: Iri::new("urn:test:checkpoint:retain:commit"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage committed transaction");
        storage
            .commit_transaction(committed_txn)
            .expect("commit transaction");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: aborted_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(22),
                    predicate: Iri::new("urn:test:checkpoint:retain:abort"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage aborted transaction");
        storage
            .abort_transaction(aborted_txn)
            .expect("abort transaction");

        let removed = storage
            .checkpoint_wal_with_retention(1)
            .expect("checkpoint with retention should succeed");

        assert_eq!(removed, 3);
        assert_eq!(storage.wal_entries().len(), 1);
    }

    #[test]
    fn storage_metrics_snapshot_tracks_lifecycle_events() {
        let storage = InMemoryStorageEngine::new();
        let commit_txn = TxnId::new(1001);
        let abort_txn = TxnId::new(1002);

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: commit_txn,
                operations: vec![
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(31),
                        predicate: Iri::new("urn:test:metrics:commit"),
                        object: Term::Iri(Iri::new("urn:test:value")),
                    }),
                    WriteOperation::PutQuad(Quad {
                        triple: Triple {
                            subject: NodeId::new(31),
                            predicate: Iri::new("urn:test:metrics:commit:quad"),
                            object: Term::Iri(Iri::new("urn:test:value")),
                        },
                        graph_name: Some(Iri::new("urn:test:graph")),
                    }),
                    WriteOperation::DeleteKey(StorageKey {
                        index: "S",
                        components: vec![NodeId::new(9999)],
                    }),
                ],
            })
            .expect("stage commit txn");
        storage.commit_transaction(commit_txn).expect("commit txn");

        storage
            .apply_write_batch(&WriteBatch {
                txn_id: abort_txn,
                operations: vec![
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(32),
                        predicate: Iri::new("urn:test:metrics:abort"),
                        object: Term::Iri(Iri::new("urn:test:value")),
                    }),
                    WriteOperation::PutQuad(Quad {
                        triple: Triple {
                            subject: NodeId::new(32),
                            predicate: Iri::new("urn:test:metrics:abort:quad"),
                            object: Term::Iri(Iri::new("urn:test:value")),
                        },
                        graph_name: Some(Iri::new("urn:test:graph")),
                    }),
                    WriteOperation::DeleteKey(StorageKey {
                        index: "S",
                        components: vec![NodeId::new(8888)],
                    }),
                ],
            })
            .expect("stage abort txn");
        storage.abort_transaction(abort_txn).expect("abort txn");

        let _ = storage
            .checkpoint_wal_with_retention(1)
            .expect("checkpoint should succeed");

        let metrics = storage.metrics_snapshot();
        assert_eq!(metrics.staged_batches, 2);
        assert_eq!(metrics.failed_stage_batches, 0);
        assert_eq!(metrics.committed_transactions, 1);
        assert_eq!(metrics.failed_commit_transactions, 0);
        assert_eq!(metrics.committed_put_triple_operations, 1);
        assert_eq!(metrics.committed_put_quad_operations, 1);
        assert_eq!(metrics.committed_delete_key_operations, 1);
        assert_eq!(metrics.aborted_transactions, 1);
        assert_eq!(metrics.failed_abort_transactions, 0);
        assert_eq!(metrics.aborted_put_triple_operations, 1);
        assert_eq!(metrics.aborted_put_quad_operations, 1);
        assert_eq!(metrics.aborted_delete_key_operations, 1);
        assert_eq!(metrics.pending_transactions, 0);
        assert!(metrics.checkpoint_truncated_records > 0);
        assert!(metrics.wal_records > 0);
    }

    #[test]
    fn storage_metrics_snapshot_tracks_write_failures() {
        let stage_fail_storage =
            InMemoryStorageEngine::with_wal(Arc::new(FailOnPhaseWal::new(Some(WalPhase::Staged))));

        let stage_err = stage_fail_storage.apply_write_batch(&WriteBatch {
            txn_id: TxnId::new(2001),
            operations: vec![WriteOperation::PutTriple(Triple {
                subject: NodeId::new(401),
                predicate: Iri::new("urn:test:fail:stage"),
                object: Term::Iri(Iri::new("urn:test:value")),
            })],
        });
        assert_eq!(
            stage_err,
            Err(OntolithError::InvalidState("injected wal append failure"))
        );

        let stage_metrics = stage_fail_storage.metrics_snapshot();
        assert_eq!(stage_metrics.staged_batches, 0);
        assert_eq!(stage_metrics.failed_stage_batches, 1);

        let commit_fail_storage = InMemoryStorageEngine::with_wal(Arc::new(FailOnPhaseWal::new(
            Some(WalPhase::Committed),
        )));
        let commit_fail_txn = TxnId::new(2002);
        commit_fail_storage
            .apply_write_batch(&WriteBatch {
                txn_id: commit_fail_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(402),
                    predicate: Iri::new("urn:test:fail:commit"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("staged write should succeed");

        let commit_err = commit_fail_storage.commit_transaction(commit_fail_txn);
        assert_eq!(
            commit_err,
            Err(OntolithError::InvalidState("injected wal append failure"))
        );

        let commit_metrics = commit_fail_storage.metrics_snapshot();
        assert_eq!(commit_metrics.staged_batches, 1);
        assert_eq!(commit_metrics.committed_transactions, 0);
        assert_eq!(commit_metrics.failed_commit_transactions, 1);

        let abort_fail_storage =
            InMemoryStorageEngine::with_wal(Arc::new(FailOnPhaseWal::new(Some(WalPhase::Aborted))));
        let abort_fail_txn = TxnId::new(2003);
        abort_fail_storage
            .apply_write_batch(&WriteBatch {
                txn_id: abort_fail_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(403),
                    predicate: Iri::new("urn:test:fail:abort"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("staged write should succeed");

        let abort_err = abort_fail_storage.abort_transaction(abort_fail_txn);
        assert_eq!(
            abort_err,
            Err(OntolithError::InvalidState("injected wal append failure"))
        );

        let abort_metrics = abort_fail_storage.metrics_snapshot();
        assert_eq!(abort_metrics.aborted_transactions, 0);
        assert_eq!(abort_metrics.failed_abort_transactions, 1);
        assert_eq!(abort_metrics.aborted_put_triple_operations, 0);
    }

    #[test]
    fn snapshot_reads_old_version_after_later_commit() {
        let storage = InMemoryStorageEngine::new();
        let first_txn = TxnId::new(3101);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: first_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:mvcc:first"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("first stage should succeed");
        storage
            .commit_transaction(first_txn)
            .expect("first commit should succeed");
        assert_eq!(storage.committed_version(), 1);

        let snapshot = storage.snapshot_with(ConsistencyLevel::Strong, None);
        assert_eq!(snapshot.version, 1);
        assert_eq!(storage.pinned_snapshot_count(), 1);

        let second_txn = TxnId::new(3102);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: second_txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(2),
                    predicate: Iri::new("urn:test:mvcc:second"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("second stage should succeed");
        storage
            .commit_transaction(second_txn)
            .expect("second commit should succeed");
        assert_eq!(storage.committed_version(), 2);

        let snapshot_triples = storage.triples_at_version_in_txn(snapshot.version, None);
        assert_eq!(snapshot_triples.len(), 1);
        assert_eq!(snapshot_triples[0].subject, NodeId::new(1));
        assert_eq!(storage.default_graph_triples().len(), 2);

        storage.release_snapshot(snapshot.snapshot_id);
        assert_eq!(storage.pinned_snapshot_count(), 0);
    }

    #[test]
    fn named_graph_quads_are_versioned() {
        let storage = InMemoryStorageEngine::new();
        let graph = Iri::new("urn:graph:mvcc");
        let first_txn = TxnId::new(3601);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: first_txn,
                operations: vec![WriteOperation::PutQuad(Quad::in_named_graph(
                    Triple::new(
                        NodeId::new(1),
                        Iri::new("urn:test:p"),
                        Term::Iri(Iri::new("urn:test:o1")),
                    ),
                    graph.clone(),
                ))],
            })
            .expect("first quad stage should succeed");
        storage
            .commit_transaction(first_txn)
            .expect("first commit should succeed");

        let snapshot = storage.snapshot();
        assert_eq!(snapshot.version, 1);

        let second_txn = TxnId::new(3602);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: second_txn,
                operations: vec![WriteOperation::PutQuad(Quad::in_named_graph(
                    Triple::new(
                        NodeId::new(2),
                        Iri::new("urn:test:p"),
                        Term::Iri(Iri::new("urn:test:o2")),
                    ),
                    graph.clone(),
                ))],
            })
            .expect("second quad stage should succeed");
        storage
            .commit_transaction(second_txn)
            .expect("second commit should succeed");

        assert_eq!(storage.quads_at_version(snapshot.version).len(), 1);
        assert_eq!(storage.quads_at_version(2).len(), 2);
    }

    #[test]
    fn prune_versions_removes_old_versions_but_keeps_pinned_and_latest() {
        let storage = InMemoryStorageEngine::with_version_retention(16);
        for seq in 1..=5u64 {
            let txn = TxnId::new(3300 + seq as u128);
            storage
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(seq),
                        predicate: Iri::new("urn:test:mvcc:prune"),
                        object: Term::Iri(Iri::new("urn:test:value")),
                    })],
                })
                .expect("stage should succeed");
            storage
                .commit_transaction(txn)
                .expect("commit should succeed");
        }
        assert_eq!(storage.committed_version(), 5);

        let snapshot = storage.snapshot();
        assert_eq!(snapshot.version, 5);
        assert_eq!(storage.pinned_snapshot_count(), 1);

        let txn = TxnId::new(3306);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(6),
                    predicate: Iri::new("urn:test:mvcc:prune"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage should succeed");
        storage
            .commit_transaction(txn)
            .expect("commit should succeed");
        assert_eq!(storage.committed_version(), 6);

        let pruned = storage.prune_versions(1).expect("prune should succeed");
        assert_eq!(pruned, 4);
        assert_eq!(storage.pruned_version_count(), 4);

        let pinned_triples = storage.triples_at_version_in_txn(snapshot.version, None);
        assert_eq!(pinned_triples.len(), 5);
        assert_eq!(storage.default_graph_triples().len(), 6);

        // A pruned version falls back to the oldest retained version (genesis = empty).
        assert!(storage.triples_at_version_in_txn(2, None).is_empty());

        storage.release_snapshot(snapshot.snapshot_id);
        assert_eq!(storage.pinned_snapshot_count(), 0);

        let pruned = storage.prune_versions(1).expect("prune should succeed");
        assert_eq!(pruned, 1);
        assert_eq!(storage.pruned_version_count(), 5);
    }

    #[test]
    fn delete_by_key_creates_new_committed_version() {
        let storage = InMemoryStorageEngine::new();
        let txn = TxnId::new(3401);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(1),
                    predicate: Iri::new("urn:test:mvcc:delete"),
                    object: Term::Iri(Iri::new("urn:test:value")),
                })],
            })
            .expect("stage should succeed");
        storage
            .commit_transaction(txn)
            .expect("commit should succeed");
        assert_eq!(storage.committed_version(), 1);

        let removed = storage
            .delete_by_key(&StorageKey {
                index: "S",
                components: vec![NodeId::new(1)],
            })
            .expect("delete should succeed");
        assert_eq!(removed, 1);
        assert_eq!(storage.committed_version(), 2);
        assert_eq!(storage.version_count(), 2);
        assert!(storage.default_graph_triples().is_empty());

        // Deleting a missing subject must not mint a version.
        let removed = storage
            .delete_by_key(&StorageKey {
                index: "S",
                components: vec![NodeId::new(99)],
            })
            .expect("delete should succeed");
        assert_eq!(removed, 0);
        assert_eq!(storage.committed_version(), 2);
    }

    #[test]
    fn wal_replay_rebuilds_version_chain() {
        let storage = InMemoryStorageEngine::new();
        for seq in 1..=3u64 {
            let txn = TxnId::new(3500 + seq as u128);
            storage
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(seq),
                        predicate: Iri::new("urn:test:mvcc:replay"),
                        object: Term::Iri(Iri::new("urn:test:value")),
                    })],
                })
                .expect("stage should succeed");
            storage
                .commit_transaction(txn)
                .expect("commit should succeed");
        }
        assert_eq!(storage.committed_version(), 3);

        let replayed = InMemoryStorageEngine::recover_from_wal(&storage.wal_entries())
            .expect("wal replay should succeed");
        assert_eq!(replayed.committed_version(), 3);
        assert_eq!(replayed.version_count(), 3);
        assert_eq!(replayed.default_graph_triples().len(), 3);

        let snapshot = replayed.snapshot();
        assert_eq!(snapshot.version, 3);
    }

    #[test]
    fn triple_repository_reads_snapshot_version() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = InMemoryTripleRepository::new(Arc::clone(&engine));
        let first_txn = TxnId::new(3701);
        repo.insert(
            first_txn,
            Triple::new(
                NodeId::new(1),
                Iri::new("urn:test:mvcc:repo"),
                Term::Iri(Iri::new("urn:test:value")),
            ),
        )
        .expect("first insert should succeed");
        engine
            .commit_transaction(first_txn)
            .expect("first commit should succeed");

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.version, 1);

        let second_txn = TxnId::new(3702);
        repo.insert(
            second_txn,
            Triple::new(
                NodeId::new(2),
                Iri::new("urn:test:mvcc:repo"),
                Term::Iri(Iri::new("urn:test:value")),
            ),
        )
        .expect("second insert should succeed");
        engine
            .commit_transaction(second_txn)
            .expect("second commit should succeed");

        assert_eq!(repo.all().len(), 2);
        let snapshot_triples = repo.all_at_version_in_txn(snapshot.version, None);
        assert_eq!(snapshot_triples.len(), 1);
        assert_eq!(snapshot_triples[0].subject, NodeId::new(1));

        assert_eq!(
            repo.by_subject_at_version_in_txn(snapshot.version, NodeId::new(1), None)
                .len(),
            1
        );
        assert!(
            repo.by_subject_at_version_in_txn(snapshot.version, NodeId::new(2), None)
                .is_empty()
        );
        assert_eq!(
            repo.matching_at_version_in_txn(
                snapshot.version,
                None,
                Some(&Iri::new("urn:test:mvcc:repo")),
                Some(&Term::Iri(Iri::new("urn:test:value"))),
                None,
            )
            .len(),
            1
        );
        assert_eq!(
            repo.by_predicate_at_version_in_txn(2, &Iri::new("urn:test:mvcc:repo"), None)
                .len(),
            2
        );
    }

    #[test]
    fn engine_triple_repository_reads_snapshot_version() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = EngineTripleRepository::new(Arc::clone(&engine) as Arc<dyn StorageEngine>);
        let txn = TxnId::new(3711);
        repo.insert(
            txn,
            Triple::new(
                NodeId::new(7),
                Iri::new("urn:test:mvcc:engine-repo"),
                Term::Iri(Iri::new("urn:test:value")),
            ),
        )
        .expect("insert should succeed");
        engine
            .commit_transaction(txn)
            .expect("commit should succeed");

        let snapshot = engine.snapshot();
        assert_eq!(repo.all_at_version_in_txn(snapshot.version, None).len(), 1);
        assert_eq!(
            repo.by_subject_at_version_in_txn(snapshot.version, NodeId::new(7), None)
                .len(),
            1
        );
    }

    #[test]
    fn quad_repository_reads_snapshot_version() {
        let engine = Arc::new(InMemoryStorageEngine::new());
        let repo = InMemoryQuadRepository::new(Arc::clone(&engine));
        let graph = Iri::new("urn:graph:mvcc:repo");
        let first_txn = TxnId::new(3721);
        repo.insert(
            first_txn,
            Quad::in_named_graph(
                Triple::new(
                    NodeId::new(1),
                    Iri::new("urn:test:p"),
                    Term::Iri(Iri::new("urn:test:o1")),
                ),
                graph.clone(),
            ),
        )
        .expect("first quad insert should succeed");
        engine
            .commit_transaction(first_txn)
            .expect("first commit should succeed");

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.version, 1);

        let second_txn = TxnId::new(3722);
        repo.insert(
            second_txn,
            Quad::in_named_graph(
                Triple::new(
                    NodeId::new(2),
                    Iri::new("urn:test:p"),
                    Term::Iri(Iri::new("urn:test:o2")),
                ),
                graph.clone(),
            ),
        )
        .expect("second quad insert should succeed");
        engine
            .commit_transaction(second_txn)
            .expect("second commit should succeed");

        assert_eq!(repo.all().len(), 2);
        assert_eq!(repo.all_at_version(snapshot.version).len(), 1);
        assert_eq!(
            repo.by_graph_name_at_version(snapshot.version, &graph)
                .len(),
            1
        );
        assert_eq!(repo.by_graph_name_at_version(2, &graph).len(), 2);
        assert!(
            repo.by_graph_name_at_version(snapshot.version, &Iri::new("urn:graph:other"))
                .is_empty()
        );
    }

    /// R1 gate: idempotent-write verification.
    /// PutTriple/PutQuad are set-semantic: re-inserting an existing statement
    /// (same batch, later batch, or replay after commit) must not duplicate.
    #[test]
    fn idempotent_put_set_semantics_no_duplicates() {
        let storage = InMemoryStorageEngine::new();
        let triple = Triple::new(
            NodeId::new(10),
            Iri::new("urn:test:knows"),
            Term::Iri(Iri::new("urn:test:bob")),
        );

        // Same triple staged twice inside one batch.
        let first = TxnId::new(101);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: first,
                operations: vec![
                    WriteOperation::PutTriple(triple.clone()),
                    WriteOperation::PutTriple(triple.clone()),
                ],
            })
            .expect("stage duplicate put");
        storage
            .commit_transaction(first)
            .expect("commit duplicate put");
        assert_eq!(storage.default_graph_triples().len(), 1);

        // Replay of the same statement in a later txn must stay a no-op.
        let replay = TxnId::new(102);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: replay,
                operations: vec![WriteOperation::PutTriple(triple.clone())],
            })
            .expect("stage replay");
        storage.commit_transaction(replay).expect("commit replay");
        assert_eq!(
            storage.default_graph_triples().len(),
            1,
            "set semantics must dedup replayed puts"
        );
        assert_eq!(storage.default_graph_triples()[0], triple);
    }

    /// Double-commit of the same txn id must be refused and never duplicate.
    #[test]
    fn idempotent_double_commit_refused_no_duplication() {
        let storage = InMemoryStorageEngine::new();
        let txn = TxnId::new(200);
        let triple = Triple::new(
            NodeId::new(20),
            Iri::new("urn:test:p"),
            Term::Iri(Iri::new("urn:test:o")),
        );
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(triple)],
            })
            .expect("stage");
        storage.commit_transaction(txn).expect("commit");
        assert!(storage.commit_transaction(txn).is_err());
        assert_eq!(storage.default_graph_triples().len(), 1);
    }

    /// DeleteTriple/DeleteQuad are idempotent: deleting an absent statement
    /// is a no-op, and a delete-after-delete leaves the store empty.
    #[test]
    fn idempotent_delete_absent_is_noop() {
        let storage = InMemoryStorageEngine::new();
        let triple = Triple::new(
            NodeId::new(30),
            Iri::new("urn:test:never"),
            Term::Iri(Iri::new("urn:test:inserted")),
        );

        // Delete of an absent triple commits cleanly and changes nothing.
        let absent = TxnId::new(301);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: absent,
                operations: vec![WriteOperation::DeleteTriple(triple.clone())],
            })
            .expect("stage absent delete");
        storage
            .commit_transaction(absent)
            .expect("commit absent delete");
        assert!(storage.default_graph_triples().is_empty());

        // Put -> delete -> delete again.
        let put = TxnId::new(302);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: put,
                operations: vec![WriteOperation::PutTriple(triple.clone())],
            })
            .expect("stage put");
        storage.commit_transaction(put).expect("commit put");
        assert_eq!(storage.default_graph_triples().len(), 1);

        let delete = TxnId::new(303);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: delete,
                operations: vec![WriteOperation::DeleteTriple(triple.clone())],
            })
            .expect("stage delete");
        storage.commit_transaction(delete).expect("commit delete");
        assert!(storage.default_graph_triples().is_empty());

        let delete_again = TxnId::new(304);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: delete_again,
                operations: vec![WriteOperation::DeleteTriple(triple.clone())],
            })
            .expect("stage delete again");
        storage
            .commit_transaction(delete_again)
            .expect("commit delete again");
        assert!(storage.default_graph_triples().is_empty());
    }

    /// Quad put dedup + delete idempotency (named graph, set semantics).
    #[test]
    fn idempotent_quad_set_semantics_and_delete() {
        let storage = InMemoryStorageEngine::new();
        let graph = Iri::new("urn:graph:set");
        let quad = Quad::in_named_graph(
            Triple::new(
                NodeId::new(40),
                Iri::new("urn:test:p"),
                Term::Iri(Iri::new("urn:test:o")),
            ),
            graph.clone(),
        );

        let first = TxnId::new(401);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: first,
                operations: vec![
                    WriteOperation::PutQuad(quad.clone()),
                    WriteOperation::PutQuad(quad.clone()),
                ],
            })
            .expect("stage duplicate quad put");
        storage.commit_transaction(first).expect("commit");
        assert_eq!(
            storage
                .quads_matching_in_graph(&graph, None, None, None, None)
                .len(),
            1
        );

        let delete = TxnId::new(402);
        storage
            .apply_write_batch(&WriteBatch {
                txn_id: delete,
                operations: vec![
                    WriteOperation::DeleteQuad(quad.clone()),
                    WriteOperation::DeleteQuad(quad.clone()),
                ],
            })
            .expect("stage duplicate quad delete");
        storage.commit_transaction(delete).expect("commit");
        assert!(
            storage
                .quads_matching_in_graph(&graph, None, None, None, None)
                .is_empty()
        );
    }
}
