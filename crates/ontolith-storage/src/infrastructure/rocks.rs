//! RocksDB durable adapter (L2).
//!
//! Vendor types stay inside this module. Public API is only Ontolith traits.

use crate::application::{DictionaryCodec, StorageEngine, WriteAheadLog};
use crate::domain::{
    SnapshotRef, StorageKey, StorageStats, WalPhase, WalRecord, WriteBatch, WriteOperation,
};
use crate::infrastructure::codec::{
    decode_quad, decode_triple, decode_u64, decode_wal_record, encode_quad, encode_triple,
    encode_u64, encode_wal_record,
};
use crate::infrastructure::indexes::{GraphIndex, TripleIndexes, quad_key, triple_key};
use ontolith_core::domain::{ConsistencyLevel, Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_transaction::domain::TxnId;
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options, WriteBatch as RocksBatch};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const CF_META: &str = "meta";
const CF_DICT_FWD: &str = "dict_fwd";
const CF_DICT_REV: &str = "dict_rev";
const CF_TRIPLES: &str = "triples";
const CF_QUADS: &str = "quads";
const CF_WAL: &str = "wal";

const META_NEXT_NODE: &[u8] = b"next_node_id";
const META_WAL_SEQ: &[u8] = b"wal_seq";
const META_DICT_EPOCH: &[u8] = b"dict_epoch";

struct EngineState {
    default_graph: Vec<Triple>,
    indexes: TripleIndexes,
    graph_index: GraphIndex,
    pending_writes: HashMap<TxnId, Vec<WriteOperation>>,
}

pub struct RocksDbStorageEngine {
    db: Arc<DB>,
    path: PathBuf,
    state: RwLock<EngineState>,
    /// Serialize durable commits against shared DB.
    commit_lock: Mutex<()>,
    next_snapshot_id: AtomicU64,
    next_node_id: AtomicU64,
    dict_epoch: AtomicU64,
    wal_seq: AtomicU64,
    staged_batches_count: AtomicU64,
    failed_stage_batches_count: AtomicU64,
    committed_txn_count: AtomicU64,
    failed_commit_txn_count: AtomicU64,
    aborted_txn_count: AtomicU64,
}

impl RocksDbStorageEngine {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OntolithError> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)
            .map_err(|e| OntolithError::Failed(format!("create dir: {e}")))?;

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cfs = [
            CF_META,
            CF_DICT_FWD,
            CF_DICT_REV,
            CF_TRIPLES,
            CF_QUADS,
            CF_WAL,
        ]
        .into_iter()
        .map(|name| ColumnFamilyDescriptor::new(name, Options::default()))
        .collect::<Vec<_>>();

        let db = DB::open_cf_descriptors(&opts, &path, cfs).map_err(rocks_err)?;
        let db = Arc::new(db);

        let mut engine = Self {
            db,
            path,
            state: RwLock::new(EngineState {
                default_graph: Vec::new(),
                indexes: TripleIndexes::default(),
                graph_index: GraphIndex::default(),
                pending_writes: HashMap::new(),
            }),
            commit_lock: Mutex::new(()),
            next_snapshot_id: AtomicU64::new(1),
            next_node_id: AtomicU64::new(1),
            dict_epoch: AtomicU64::new(0),
            wal_seq: AtomicU64::new(0),
            staged_batches_count: AtomicU64::new(0),
            failed_stage_batches_count: AtomicU64::new(0),
            committed_txn_count: AtomicU64::new(0),
            failed_commit_txn_count: AtomicU64::new(0),
            aborted_txn_count: AtomicU64::new(0),
        };
        engine.load_meta()?;
        engine.rebuild_memory_from_disk()?;
        Ok(engine)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn cf(&self, name: &str) -> Result<&rocksdb::ColumnFamily, OntolithError> {
        self.db
            .cf_handle(name)
            .ok_or(OntolithError::Storage("missing column family"))
    }

    fn load_meta(&mut self) -> Result<(), OntolithError> {
        let cf = self.cf(CF_META)?;
        if let Some(v) = self.db.get_cf(cf, META_NEXT_NODE).map_err(rocks_err)? {
            self.next_node_id.store(decode_u64(&v)?, Ordering::SeqCst);
        }
        if let Some(v) = self.db.get_cf(cf, META_WAL_SEQ).map_err(rocks_err)? {
            self.wal_seq.store(decode_u64(&v)?, Ordering::SeqCst);
        }
        if let Some(v) = self.db.get_cf(cf, META_DICT_EPOCH).map_err(rocks_err)? {
            self.dict_epoch.store(decode_u64(&v)?, Ordering::SeqCst);
        }
        Ok(())
    }

    fn rebuild_memory_from_disk(&self) -> Result<(), OntolithError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("storage state lock poisoned"))?;
        state.default_graph.clear();
        state.indexes.clear();
        state.graph_index.clear();

        let cf_t = self.cf(CF_TRIPLES)?;
        let iter = self.db.iterator_cf(cf_t, IteratorMode::Start);
        for item in iter {
            let (_k, v) = item.map_err(rocks_err)?;
            let triple = decode_triple(&v)?;
            if state.indexes.insert(&triple) {
                state.default_graph.push(triple);
            }
        }

        let cf_q = self.cf(CF_QUADS)?;
        let iter = self.db.iterator_cf(cf_q, IteratorMode::Start);
        for item in iter {
            let (_k, v) = item.map_err(rocks_err)?;
            let quad = decode_quad(&v)?;
            let _ = state.graph_index.insert(&quad);
        }
        Ok(())
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
                    if subject_ok && predicate_ok && object_ok {
                        if let Some(pos) = triples.iter().position(|t| t == triple) {
                            triples[pos] = triple.clone();
                        } else {
                            triples.push(triple.clone());
                        }
                    }
                }
                WriteOperation::DeleteTriple(triple) => {
                    triples.retain(|t| t != triple);
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

    fn apply_memory_op(state: &mut EngineState, op: &WriteOperation) {
        match op {
            WriteOperation::PutTriple(triple) => {
                if state.indexes.insert(triple) {
                    state.default_graph.push(triple.clone());
                }
            }
            WriteOperation::PutQuad(quad) => {
                let _ = state.graph_index.insert(quad);
            }
            WriteOperation::DeleteTriple(triple) => {
                if state.indexes.remove_exact(triple) {
                    state.default_graph.retain(|t| t != triple);
                }
            }
            WriteOperation::DeleteQuad(quad) => {
                let _ = state.graph_index.remove_exact(quad);
            }
            WriteOperation::DeleteKey(key) => {
                if let Some(subject_id) = key.components.first().copied() {
                    let _ = state.indexes.remove_by_subject(subject_id);
                    state.default_graph.retain(|t| t.subject != subject_id);
                    let _ = state.graph_index.remove_by_subject(subject_id);
                }
            }
        }
    }

    fn durable_apply_ops(
        &self,
        batch: &mut RocksBatch,
        operations: &[WriteOperation],
    ) -> Result<(), OntolithError> {
        let cf_t = self.cf(CF_TRIPLES)?;
        let cf_q = self.cf(CF_QUADS)?;
        for op in operations {
            match op {
                WriteOperation::PutTriple(t) => {
                    let k = triple_key(t);
                    batch.put_cf(cf_t, k, encode_triple(t));
                }
                WriteOperation::DeleteTriple(t) => {
                    batch.delete_cf(cf_t, triple_key(t));
                }
                WriteOperation::PutQuad(q) => {
                    batch.put_cf(cf_q, quad_key(q), encode_quad(q));
                }
                WriteOperation::DeleteQuad(q) => {
                    batch.delete_cf(cf_q, quad_key(q));
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied() {
                        // Read current memory view under commit lock for keys to delete.
                        let state = self
                            .state
                            .read()
                            .map_err(|_| OntolithError::InvalidState("lock poisoned"))?;
                        let doomed: Vec<Triple> = state
                            .default_graph
                            .iter()
                            .filter(|t| t.subject == subject_id)
                            .cloned()
                            .collect();
                        let doomed_q: Vec<Quad> = state
                            .graph_index
                            .all
                            .iter()
                            .filter(|q| q.triple.subject == subject_id)
                            .cloned()
                            .collect();
                        drop(state);
                        for t in doomed {
                            batch.delete_cf(cf_t, triple_key(&t));
                        }
                        for q in doomed_q {
                            batch.delete_cf(cf_q, quad_key(&q));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn append_wal_record(
        &self,
        batch: &mut RocksBatch,
        rec: &WalRecord,
    ) -> Result<(), OntolithError> {
        let seq = self.wal_seq.fetch_add(1, Ordering::SeqCst);
        let cf = self.cf(CF_WAL)?;
        let cf_meta = self.cf(CF_META)?;
        batch.put_cf(cf, encode_u64(seq), encode_wal_record(rec));
        batch.put_cf(cf_meta, META_WAL_SEQ, encode_u64(seq + 1));
        Ok(())
    }
}

impl DictionaryCodec for RocksDbStorageEngine {
    fn encode_node(&self, value: &str) -> NodeId {
        let cf_fwd = self.cf(CF_DICT_FWD).expect("cf");
        if let Ok(Some(raw)) = self.db.get_cf(cf_fwd, value.as_bytes())
            && let Ok(id) = decode_u64(&raw)
        {
            return NodeId::new(id);
        }
        let id = self.next_node_id.fetch_add(1, Ordering::SeqCst);
        let node = NodeId::new(id);
        let cf_rev = self.cf(CF_DICT_REV).expect("cf");
        let cf_meta = self.cf(CF_META).expect("cf");
        let mut batch = RocksBatch::default();
        batch.put_cf(cf_fwd, value.as_bytes(), encode_u64(id));
        batch.put_cf(cf_rev, encode_u64(id), value.as_bytes());
        batch.put_cf(cf_meta, META_NEXT_NODE, encode_u64(id + 1));
        let _ = self.db.write(batch);
        node
    }

    fn decode_node(&self, node_id: NodeId) -> Option<String> {
        let cf = self.cf(CF_DICT_REV).ok()?;
        let raw = self.db.get_cf(cf, encode_u64(node_id.get())).ok()??;
        String::from_utf8(raw).ok()
    }

    fn len(&self) -> usize {
        let cf = match self.cf(CF_DICT_FWD) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        self.db.iterator_cf(cf, IteratorMode::Start).count()
    }

    fn contains_value(&self, value: &str) -> bool {
        let cf = match self.cf(CF_DICT_FWD) {
            Ok(c) => c,
            Err(_) => return false,
        };
        matches!(self.db.get_cf(cf, value.as_bytes()), Ok(Some(_)))
    }

    fn epoch(&self) -> u64 {
        self.dict_epoch.load(Ordering::SeqCst)
    }
}

impl WriteAheadLog for RocksDbStorageEngine {
    fn append(&self, record: WalRecord) -> Result<(), OntolithError> {
        let _guard = self
            .commit_lock
            .lock()
            .map_err(|_| OntolithError::InvalidState("commit lock poisoned"))?;
        let mut batch = RocksBatch::default();
        self.append_wal_record(&mut batch, &record)?;
        self.db.write(batch).map_err(rocks_err)
    }

    fn entries(&self) -> Vec<WalRecord> {
        let cf = match self.cf(CF_WAL) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for item in self.db.iterator_cf(cf, IteratorMode::Start) {
            if let Ok((_k, v)) = item
                && let Ok(rec) = decode_wal_record(&v)
            {
                out.push(rec);
            }
        }
        out
    }

    fn truncate_prefix(&self, upto_exclusive: usize) -> Result<(), OntolithError> {
        let _guard = self
            .commit_lock
            .lock()
            .map_err(|_| OntolithError::InvalidState("commit lock poisoned"))?;
        let cf = self.cf(CF_WAL)?;
        let keys: Vec<Vec<u8>> = self
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .take(upto_exclusive)
            .filter_map(|i| i.ok().map(|(k, _)| k.to_vec()))
            .collect();
        let mut batch = RocksBatch::default();
        for k in keys {
            batch.delete_cf(cf, k);
        }
        self.db.write(batch).map_err(rocks_err)
    }
}

impl StorageEngine for RocksDbStorageEngine {
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

        // Durable staged marker (operations stored in WAL payload).
        let rec = WalRecord {
            txn_id: batch.txn_id,
            phase: WalPhase::Staged,
            operation_count: batch.operations.len(),
            operations: batch.operations.clone(),
        };
        drop(guard);
        if let Err(err) = WriteAheadLog::append(self, rec) {
            self.failed_stage_batches_count
                .fetch_add(1, Ordering::SeqCst);
            return Err(err);
        }
        self.staged_batches_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn commit_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OntolithError::InvalidState("commit lock poisoned"))?;

        let operations = {
            let mut guard = self.state.write().map_err(|_| {
                self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
                OntolithError::InvalidState("storage state lock poisoned")
            })?;
            match guard.pending_writes.remove(&txn_id) {
                Some(ops) => ops,
                None => {
                    self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
                    return Err(OntolithError::InvalidState(
                        "pending storage transaction not found",
                    ));
                }
            }
        };

        let mut rocks_batch = RocksBatch::default();
        // Note: DeleteKey needs pre-image from memory before memory apply.
        self.durable_apply_ops(&mut rocks_batch, &operations)?;
        self.append_wal_record(
            &mut rocks_batch,
            &WalRecord {
                txn_id,
                phase: WalPhase::Committed,
                operation_count: 0,
                operations: Vec::new(),
            },
        )?;
        self.db.write(rocks_batch).map_err(|e| {
            self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
            rocks_err(e)
        })?;

        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("storage state lock poisoned"))?;
        for op in &operations {
            Self::apply_memory_op(&mut guard, op);
        }
        self.committed_txn_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn abort_transaction(&self, txn_id: TxnId) -> Result<(), OntolithError> {
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("storage state lock poisoned"))?;
        let removed = guard.pending_writes.remove(&txn_id);
        drop(guard);
        if removed.is_some() {
            WriteAheadLog::append(
                self,
                WalRecord {
                    txn_id,
                    phase: WalPhase::Aborted,
                    operation_count: 0,
                    operations: Vec::new(),
                },
            )?;
            self.aborted_txn_count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn delete_by_key(&self, key: &StorageKey) -> Result<usize, OntolithError> {
        // Immediate durable subject delete outside txn (admin path).
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OntolithError::InvalidState("commit lock poisoned"))?;
        if key.components.is_empty() {
            return Ok(0);
        }
        let mut rocks_batch = RocksBatch::default();
        let op = WriteOperation::DeleteKey(key.clone());
        self.durable_apply_ops(&mut rocks_batch, std::slice::from_ref(&op))?;
        self.db.write(rocks_batch).map_err(rocks_err)?;
        let mut guard = self
            .state
            .write()
            .map_err(|_| OntolithError::InvalidState("lock poisoned"))?;
        let before = guard.default_graph.len() + guard.graph_index.all.len();
        Self::apply_memory_op(&mut guard, &op);
        let after = guard.default_graph.len() + guard.graph_index.all.len();
        Ok(before.saturating_sub(after))
    }

    fn snapshot_with(
        &self,
        consistency: ConsistencyLevel,
        read_txn_id: Option<TxnId>,
    ) -> SnapshotRef {
        SnapshotRef {
            snapshot_id: self.next_snapshot_id.fetch_add(1, Ordering::SeqCst),
            read_txn_id,
            consistency,
        }
    }

    fn stats(&self) -> StorageStats {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return StorageStats::default(),
        };
        let (subjects, predicates, objects) = guard.indexes.distinct_counts();
        StorageStats {
            triple_count: guard.default_graph.len() as u64,
            quad_count: guard.graph_index.all.len() as u64,
            distinct_subjects: subjects,
            distinct_predicates: predicates,
            distinct_objects: objects,
            named_graph_count: guard.graph_index.by_graph.len() as u64,
            dictionary_entries: self.len() as u64,
            pending_transactions: guard.pending_writes.len() as u64,
            wal_records: self.entries().len() as u64,
            index_kinds_active: 6,
        }
    }

    fn default_graph_triples_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut triples = guard.default_graph.clone();
        if let Some(txn_id) = txn_id
            && let Some(ops) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, ops, None, None, None);
        }
        triples
    }

    fn triples_by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut triples = guard.indexes.by_subject(subject);
        if let Some(txn_id) = txn_id
            && let Some(ops) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, ops, Some(subject), None, None);
        }
        triples
    }

    fn triples_by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut triples = guard.indexes.by_predicate(predicate);
        if let Some(txn_id) = txn_id
            && let Some(ops) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, ops, None, Some(predicate), None);
        }
        triples
    }

    fn triples_by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple> {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut triples = guard.indexes.by_object(object);
        if let Some(txn_id) = txn_id
            && let Some(ops) = guard.pending_writes.get(&txn_id)
        {
            Self::apply_ops_to_triple_projection(&mut triples, ops, None, None, Some(object));
        }
        triples
    }

    fn named_graph_quads(&self) -> Vec<Quad> {
        self.state
            .read()
            .map(|s| s.graph_index.all.clone())
            .unwrap_or_default()
    }

    fn quads_by_graph_in_txn(&self, graph_name: Option<&Iri>, txn_id: Option<TxnId>) -> Vec<Quad> {
        let guard = match self.state.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut quads = match graph_name {
            Some(g) => guard.graph_index.by_graph_name(g),
            None => guard
                .default_graph
                .iter()
                .cloned()
                .map(Quad::in_default_graph)
                .collect(),
        };
        if let Some(txn_id) = txn_id
            && let Some(ops) = guard.pending_writes.get(&txn_id)
        {
            for op in ops {
                match op {
                    WriteOperation::PutQuad(q)
                        if graph_name
                            .map(|g| q.graph_name.as_ref() == Some(g))
                            .unwrap_or(q.graph_name.is_none()) =>
                    {
                        if !quads.iter().any(|x| x == q) {
                            quads.push(q.clone());
                        }
                    }
                    WriteOperation::DeleteQuad(q) => {
                        quads.retain(|x| x != q);
                    }
                    _ => {}
                }
            }
        }
        quads
    }
}

/// Open a durable engine at `path` (creates directory if needed).
pub fn open_rocksdb_engine(path: impl AsRef<Path>) -> Result<RocksDbStorageEngine, OntolithError> {
    RocksDbStorageEngine::open(path)
}

fn rocks_err(err: rocksdb::Error) -> OntolithError {
    OntolithError::Failed(format!("rocksdb: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WriteOperation;
    use ontolith_rdf::domain::Term;
    use ontolith_transaction::domain::TxnId;

    #[test]
    fn rocksdb_commit_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        {
            let engine = RocksDbStorageEngine::open(&path).expect("open");
            let dict_id = engine.encode_node("http://ex.org/alice");
            assert_eq!(
                engine.decode_node(dict_id).as_deref(),
                Some("http://ex.org/alice")
            );

            let txn = TxnId::new(1);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(1),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o")),
                    })],
                })
                .unwrap();
            engine.commit_transaction(txn).unwrap();
            assert_eq!(engine.stats().triple_count, 1);
        }
        // reopen
        let engine = RocksDbStorageEngine::open(&path).expect("reopen");
        assert_eq!(engine.stats().triple_count, 1);
        assert_eq!(engine.default_graph_triples().len(), 1);
        assert_eq!(
            engine.decode_node(NodeId::new(1)).as_deref(),
            Some("http://ex.org/alice")
        );
        assert_eq!(
            engine
                .triples_by_predicate_in_txn(&Iri::new("urn:p"), None)
                .len(),
            1
        );
    }

    #[test]
    fn rocksdb_abort_discards_pending() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        let txn = TxnId::new(9);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(Triple {
                    subject: NodeId::new(3),
                    predicate: Iri::new("urn:p"),
                    object: Term::Iri(Iri::new("urn:o")),
                })],
            })
            .unwrap();
        assert_eq!(engine.default_graph_triples_in_txn(Some(txn)).len(), 1);
        engine.abort_transaction(txn).unwrap();
        assert!(engine.default_graph_triples().is_empty());
        // reopen still empty
        drop(engine);
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        assert!(engine.default_graph_triples().is_empty());
    }

    #[test]
    fn rocksdb_exact_delete_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let engine = RocksDbStorageEngine::open(path).unwrap();
        let t = Triple {
            subject: NodeId::new(1),
            predicate: Iri::new("urn:p"),
            object: Term::Iri(Iri::new("urn:o")),
        };
        let txn = TxnId::new(1);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(t.clone())],
            })
            .unwrap();
        engine.commit_transaction(txn).unwrap();
        let del = TxnId::new(2);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: del,
                operations: vec![WriteOperation::DeleteTriple(t)],
            })
            .unwrap();
        engine.commit_transaction(del).unwrap();
        drop(engine);
        let engine = RocksDbStorageEngine::open(path).unwrap();
        assert_eq!(engine.stats().triple_count, 0);
    }
}
