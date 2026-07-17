use crate::application::{
    DictionaryCodec, QuadRepository, StorageEngine, TripleRepository, WriteAheadLog,
};
use crate::domain::{SnapshotRef, StorageKey, WalPhase, WalRecord, WriteBatch, WriteOperation};
use ontolith_core::domain::{Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Triple};
use ontolith_transaction::domain::TxnId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

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
}

#[derive(Default)]
struct StorageState {
    default_graph: Vec<Triple>,
    spo_index: HashMap<NodeId, Vec<Triple>>,
    named_graph_quads: Vec<Quad>,
    pending_writes: HashMap<TxnId, Vec<WriteOperation>>,
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
}

impl InMemoryStorageEngine {
    pub fn new() -> Self {
        Self::with_wal(Arc::new(InMemoryWal::new()))
    }

    pub fn with_wal(wal: Arc<dyn WriteAheadLog>) -> Self {
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
        let pending_transactions = self
            .state
            .read()
            .map(|state| state.pending_writes.len())
            .unwrap_or(0);

        StorageMetricsSnapshot {
            staged_batches: self.staged_batches_count.load(Ordering::SeqCst),
            failed_stage_batches: self.failed_stage_batches_count.load(Ordering::SeqCst),
            committed_transactions: self.committed_txn_count.load(Ordering::SeqCst),
            failed_commit_transactions: self.failed_commit_txn_count.load(Ordering::SeqCst),
            committed_put_triple_operations: self
                .committed_put_triple_ops_count
                .load(Ordering::SeqCst),
            committed_put_quad_operations: self
                .committed_put_quad_ops_count
                .load(Ordering::SeqCst),
            committed_delete_key_operations: self
                .committed_delete_key_ops_count
                .load(Ordering::SeqCst),
            aborted_transactions: self.aborted_txn_count.load(Ordering::SeqCst),
            failed_abort_transactions: self.failed_abort_txn_count.load(Ordering::SeqCst),
            aborted_put_triple_operations: self
                .aborted_put_triple_ops_count
                .load(Ordering::SeqCst),
            aborted_put_quad_operations: self
                .aborted_put_quad_ops_count
                .load(Ordering::SeqCst),
            aborted_delete_key_operations: self
                .aborted_delete_key_ops_count
                .load(Ordering::SeqCst),
            checkpoint_truncated_records: self
                .checkpoint_truncated_count
                .load(Ordering::SeqCst),
            pending_transactions,
            wal_records: self.wal.entries().len(),
        }
    }

    pub fn recover_from_wal(records: &[WalRecord]) -> Result<Self, OntolithError> {
        Self::recover_internal(records, false)
    }

    pub fn recover_from_wal_tolerant(records: &[WalRecord]) -> Result<Self, OntolithError> {
        Self::recover_internal(records, true)
    }

    fn recover_internal(
        records: &[WalRecord],
        tolerant: bool,
    ) -> Result<Self, OntolithError> {
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

                    for op in operations {
                        Self::apply_committed_operation(&mut state, op);
                    }
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
            wal,
        })
    }

    fn remove_by_subject(state: &mut StorageState, subject_id: NodeId) -> usize {
        let before_default = state.default_graph.len();
        state.default_graph.retain(|t| t.subject != subject_id);
        let removed_default = before_default - state.default_graph.len();

        let before_quads = state.named_graph_quads.len();
        state
            .named_graph_quads
            .retain(|q| q.triple.subject != subject_id);
        let removed_quads = before_quads - state.named_graph_quads.len();

        removed_default + removed_quads
    }

    fn rebuild_spo_index(state: &mut StorageState) {
        state.spo_index.clear();
        for triple in &state.default_graph {
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
    ) {
        for op in operations {
            match op {
                WriteOperation::PutTriple(triple) => {
                    if subject_filter.is_none_or(|subject| subject == triple.subject) {
                        triples.push(triple.clone());
                    }
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied()
                        && subject_filter.is_none_or(|subject| subject == subject_id)
                    {
                        triples.retain(|existing| existing.subject != subject_id);
                    }
                }
                WriteOperation::PutQuad(_) => {}
            }
        }
    }

    fn apply_committed_operation(state: &mut StorageState, op: WriteOperation) {
        match op {
            WriteOperation::PutTriple(triple) => {
                state.default_graph.push(triple);
            }
            WriteOperation::PutQuad(quad) => {
                state.named_graph_quads.push(quad);
            }
            WriteOperation::DeleteKey(key) => {
                if let Some(subject_id) = key.components.first().copied() {
                    let _ = Self::remove_by_subject(state, subject_id);
                }
            }
        }
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
            self.failed_commit_txn_count
                .fetch_add(1, Ordering::SeqCst);
            OntolithError::InvalidState("storage state lock poisoned")
        })?;

        let Some(operations) = guard.pending_writes.remove(&txn_id) else {
            self.failed_commit_txn_count
                .fetch_add(1, Ordering::SeqCst);
            return Err(OntolithError::InvalidState(
                "pending storage transaction not found",
            ));
        };

        let mut put_triple_ops = 0u64;
        let mut put_quad_ops = 0u64;
        let mut delete_key_ops = 0u64;

        for op in operations {
            match &op {
                WriteOperation::PutTriple(_) => put_triple_ops += 1,
                WriteOperation::PutQuad(_) => put_quad_ops += 1,
                WriteOperation::DeleteKey(_) => delete_key_ops += 1,
            }
            Self::apply_committed_operation(&mut guard, op);
        }
        Self::rebuild_spo_index(&mut guard);

        if let Err(err) = self.wal.append(WalRecord {
            txn_id,
            phase: WalPhase::Committed,
            operation_count: 0,
            operations: Vec::new(),
        }) {
            self.failed_commit_txn_count
                .fetch_add(1, Ordering::SeqCst);
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
            self.failed_abort_txn_count
                .fetch_add(1, Ordering::SeqCst);
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
                    WriteOperation::DeleteKey(_) => delete_key_ops += 1,
                }
            }

            if let Err(err) = self.wal.append(WalRecord {
                txn_id,
                phase: WalPhase::Aborted,
                operation_count: ops.len(),
                operations: Vec::new(),
            }) {
                self.failed_abort_txn_count
                    .fetch_add(1, Ordering::SeqCst);
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

        let removed = Self::remove_by_subject(&mut guard, subject_id);
        Self::rebuild_spo_index(&mut guard);
        Ok(removed)
    }

    fn snapshot(&self) -> SnapshotRef {
        let snapshot_id = self.next_snapshot_id.fetch_add(1, Ordering::SeqCst);
        SnapshotRef {
            snapshot_id,
            read_txn_id: None,
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

        let mut triples = guard.default_graph.clone();
        if let Some(txn_id) = txn_id
            && let Some(operations) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, operations, None);
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
            Self::apply_ops_to_triple_projection(&mut triples, operations, Some(subject));
        }
        triples
    }

    fn named_graph_quads(&self) -> Vec<Quad> {
        self.state
            .read()
            .map(|s| s.named_graph_quads.clone())
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

    fn by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        self.engine.triples_by_subject_in_txn(subject, txn_id)
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
}

pub fn status() -> &'static str {
    "infrastructure"
}

#[cfg(test)]
mod tests {
    use super::{
        InMemoryDictionary, InMemoryQuadRepository, InMemoryStorageEngine, InMemoryTripleRepository,
    };
    use crate::application::{
        DictionaryCodec, QuadRepository, StorageEngine, TransactionalWriteService,
        TripleRepository, WriteAheadLog,
    };
    use crate::domain::{StorageKey, WalPhase, WalRecord, WriteBatch, WriteOperation};
    use ontolith_core::error::OntolithError;
    use ontolith_core::domain::{Iri, NodeId};
    use ontolith_rdf::domain::{Quad, Term, Triple};
    use std::sync::Arc;
    use std::sync::RwLock;
    use ontolith_transaction::domain::{TxnId, TxnMode};
    use ontolith_transaction::infrastructure::InMemoryTransactionManager;

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
            self.records.read().map(|records| records.clone()).unwrap_or_default()
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
        assert_eq!(dictionary.decode_node(id_a).as_deref(), Some("urn:test:alice"));
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

        storage.apply_write_batch(&batch).expect("write batch must succeed");
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
        assert!(repo.by_subject_in_txn(subject, Some(TxnId::new(999))).is_empty());

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
        assert_eq!(repo.by_subject_in_txn(NodeId::new(99), Some(pending_txn)).len(), 1);
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
        storage
            .commit_transaction(commit_txn)
            .expect("commit txn");

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
        storage
            .abort_transaction(abort_txn)
            .expect("abort txn");

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
        let stage_fail_storage = InMemoryStorageEngine::with_wal(Arc::new(FailOnPhaseWal::new(
            Some(WalPhase::Staged),
        )));

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

        let abort_fail_storage = InMemoryStorageEngine::with_wal(Arc::new(FailOnPhaseWal::new(
            Some(WalPhase::Aborted),
        )));
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
}
