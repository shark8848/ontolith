//! RocksDB durable adapter (L2).
//!
//! Vendor types stay inside this module. Public API is only Ontolith traits.

use crate::application::{DictionaryCodec, StorageEngine, WriteAheadLog};
use crate::domain::{
    SnapshotRef, StorageKey, StorageStats, WalPhase, WalRecord, WriteBatch, WriteOperation,
    encode_osp_key, encode_osp_object_prefix, encode_pos_key, encode_pos_predicate_prefix,
    encode_spo_key, encode_spo_subject_prefix,
};
use crate::infrastructure::codec::{
    decode_quad, decode_triple, decode_u64, decode_wal_record, encode_quad, encode_triple,
    encode_u64, encode_wal_record,
};
use crate::infrastructure::indexes::{quad_graph_prefix, quad_key, triple_key};
use ontolith_core::domain::{ConsistencyLevel, Iri, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::{Quad, Term, Triple};
use ontolith_transaction::domain::TxnId;
use rocksdb::{
    ColumnFamilyDescriptor, DB, Direction, IteratorMode, Options, WriteBatch as RocksBatch,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const CF_META: &str = "meta";
const CF_DICT_FWD: &str = "dict_fwd";
const CF_DICT_REV: &str = "dict_rev";
const CF_TRIPLES: &str = "triples";
const CF_QUADS: &str = "quads";
const CF_WAL: &str = "wal";
const CF_SPO_INDEX: &str = "spo_index";
const CF_POS_INDEX: &str = "pos_index";
const CF_OSP_INDEX: &str = "osp_index";
const CF_VERSIONS: &str = "versions";
const CF_VERSIONS_QUADS: &str = "versions_quads";

const META_NEXT_NODE: &[u8] = b"next_node_id";
const META_WAL_SEQ: &[u8] = b"wal_seq";
const META_DICT_EPOCH: &[u8] = b"dict_epoch";
const META_NEXT_VERSION: &[u8] = b"next_version";

struct EngineState {
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
    /// Next MVCC commit sequence (monotonic, includes pruned versions).
    next_version: AtomicU64,
    /// Committed versions materialized in the `versions` CFs (genesis `0` is
    /// implicit and never stored).
    retained_versions: Mutex<BTreeSet<u64>>,
    pruned_versions_count: AtomicU64,
    /// Outstanding snapshots: snapshot id → pinned committed version.
    pinned_snapshots: Mutex<HashMap<u64, u64>>,
    /// Auto-prune retention applied after each commit/delete (default 16).
    version_retention: usize,
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
            CF_SPO_INDEX,
            CF_POS_INDEX,
            CF_OSP_INDEX,
            CF_VERSIONS,
            CF_VERSIONS_QUADS,
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
                pending_writes: HashMap::new(),
            }),
            commit_lock: Mutex::new(()),
            next_snapshot_id: AtomicU64::new(1),
            next_node_id: AtomicU64::new(1),
            dict_epoch: AtomicU64::new(0),
            wal_seq: AtomicU64::new(0),
            next_version: AtomicU64::new(1),
            retained_versions: Mutex::new(BTreeSet::new()),
            pruned_versions_count: AtomicU64::new(0),
            pinned_snapshots: Mutex::new(HashMap::new()),
            version_retention: 16,
            staged_batches_count: AtomicU64::new(0),
            failed_stage_batches_count: AtomicU64::new(0),
            committed_txn_count: AtomicU64::new(0),
            failed_commit_txn_count: AtomicU64::new(0),
            aborted_txn_count: AtomicU64::new(0),
        };
        engine.ensure_index_column_families()?;
        engine.ensure_version_chain()?;
        engine.load_meta()?;
        engine.load_retained_versions()?;
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
        if let Some(v) = self.db.get_cf(cf, META_NEXT_VERSION).map_err(rocks_err)? {
            self.next_version.store(decode_u64(&v)?, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Backfill SPO/POS/OSP index column families for a database created
    /// before index CFs existed (triples present, index CFs empty).
    fn ensure_index_column_families(&self) -> Result<(), OntolithError> {
        let cf_t = self.cf(CF_TRIPLES)?;
        let cf_spo = self.cf(CF_SPO_INDEX)?;
        if self.db.iterator_cf(cf_t, IteratorMode::Start).count() == 0
            || self.db.iterator_cf(cf_spo, IteratorMode::Start).count() > 0
        {
            return Ok(());
        }
        let cf_pos = self.cf(CF_POS_INDEX)?;
        let cf_osp = self.cf(CF_OSP_INDEX)?;
        let mut batch = RocksBatch::default();
        for item in self.db.iterator_cf(cf_t, IteratorMode::Start) {
            let (_k, v) = item.map_err(rocks_err)?;
            let triple = decode_triple(&v)?;
            batch.put_cf(
                cf_spo,
                encode_spo_key(triple.subject, &triple.predicate, &triple.object),
                encode_triple(&triple),
            );
            batch.put_cf(
                cf_pos,
                encode_pos_key(&triple.predicate, &triple.object, triple.subject),
                encode_triple(&triple),
            );
            batch.put_cf(
                cf_osp,
                encode_osp_key(&triple.object, triple.subject, &triple.predicate),
                encode_triple(&triple),
            );
        }
        self.db.write(batch).map_err(rocks_err)
    }

    /// Materialize a version-1 snapshot of the current committed state for a
    /// database created before MVCC version CFs existed.
    fn ensure_version_chain(&self) -> Result<(), OntolithError> {
        let cf_meta = self.cf(CF_META)?;
        if self
            .db
            .get_cf(cf_meta, META_NEXT_VERSION)
            .map_err(rocks_err)?
            .is_some()
        {
            return Ok(());
        }
        let triples = self.scan_triples_with_prefix(CF_TRIPLES, None)?;
        let quads = self.scan_quads_with_prefix(None)?;
        if triples.is_empty() && quads.is_empty() {
            // Fresh database: genesis only, no version chain materialization.
            return Ok(());
        }
        let cf_v = self.cf(CF_VERSIONS)?;
        let cf_vq = self.cf(CF_VERSIONS_QUADS)?;
        let mut batch = RocksBatch::default();
        for triple in &triples {
            batch.put_cf(
                cf_v,
                Self::version_key(1, &triple_key(triple)),
                encode_triple(triple),
            );
        }
        for quad in &quads {
            batch.put_cf(
                cf_vq,
                Self::version_key(1, &quad_key(quad)),
                encode_quad(quad),
            );
        }
        batch.put_cf(cf_meta, META_NEXT_VERSION, encode_u64(2));
        self.db.write(batch).map_err(rocks_err)
    }

    /// Rebuild the in-memory retained-version set from the `versions` CFs
    /// (distinct big-endian version prefixes, ascending).
    fn load_retained_versions(&self) -> Result<(), OntolithError> {
        let cf = self.cf(CF_VERSIONS)?;
        let mut retained = self
            .retained_versions
            .lock()
            .map_err(|_| OntolithError::InvalidState("retained versions lock poisoned"))?;
        let mut last: Option<u64> = None;
        for item in self.db.iterator_cf(cf, IteratorMode::Start) {
            let (k, _) = item.map_err(rocks_err)?;
            if k.len() < 8 {
                continue;
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&k[..8]);
            let version = u64::from_be_bytes(arr);
            if last != Some(version) {
                retained.insert(version);
                last = Some(version);
            }
        }
        Ok(())
    }

    /// MVCC snapshot key: big-endian version prefix ‖ physical key, so
    /// versions sort ascending and prefix scans isolate one version.
    fn version_key(version: u64, key: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + key.len());
        out.extend_from_slice(&encode_u64(version));
        out.extend_from_slice(key);
        out
    }

    /// Full default-graph snapshot of a committed version from the `versions`
    /// CF (immutable; pruned versions fall back at the caller).
    fn scan_version_triples(&self, version: u64) -> Result<Vec<Triple>, OntolithError> {
        let prefix = encode_u64(version);
        let cf = self.cf(CF_VERSIONS)?;
        let mut out = Vec::new();
        for item in self
            .db
            .iterator_cf(cf, IteratorMode::From(&prefix, Direction::Forward))
        {
            let (k, v) = item.map_err(rocks_err)?;
            if !k.starts_with(&prefix) {
                break;
            }
            out.push(decode_triple(&v)?);
        }
        Ok(out)
    }

    /// Full named-graph snapshot of a committed version from the
    /// `versions_quads` CF.
    fn scan_version_quads(&self, version: u64) -> Result<Vec<Quad>, OntolithError> {
        let prefix = encode_u64(version);
        let cf = self.cf(CF_VERSIONS_QUADS)?;
        let mut out = Vec::new();
        for item in self
            .db
            .iterator_cf(cf, IteratorMode::From(&prefix, Direction::Forward))
        {
            let (k, v) = item.map_err(rocks_err)?;
            if !k.starts_with(&prefix) {
                break;
            }
            out.push(decode_quad(&v)?);
        }
        Ok(out)
    }

    /// Apply a committed write batch to the full default graph / named quads
    /// (set semantics: Put dedups, Delete removes exact, DeleteKey by subject).
    fn apply_committed_ops(
        triples: &mut Vec<Triple>,
        quads: &mut Vec<Quad>,
        operations: &[WriteOperation],
    ) {
        for op in operations {
            match op {
                WriteOperation::PutTriple(t) => {
                    if !triples.iter().any(|x| x == t) {
                        triples.push(t.clone());
                    }
                }
                WriteOperation::DeleteTriple(t) => {
                    triples.retain(|x| x != t);
                }
                WriteOperation::PutQuad(q) => {
                    if !quads.iter().any(|x| x == q) {
                        quads.push(q.clone());
                    }
                }
                WriteOperation::DeleteQuad(q) => {
                    quads.retain(|x| x != q);
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied() {
                        triples.retain(|x| x.subject != subject_id);
                        quads.retain(|x| x.triple.subject != subject_id);
                    }
                }
            }
        }
    }

    /// Batch-delete version snapshots beyond `retention` (keeps the newest
    /// committed version and any version pinned by an outstanding snapshot;
    /// genesis `0` is implicit). Returns the pruned version numbers; the
    /// caller applies the batch and then updates retention metadata.
    fn prune_locked(
        &self,
        batch: &mut RocksBatch,
        retention: usize,
    ) -> Result<Vec<u64>, OntolithError> {
        let keep = retention.max(1);
        let latest = self.next_version.load(Ordering::SeqCst).saturating_sub(1);
        let pinned: HashSet<u64> = self
            .pinned_snapshots
            .lock()
            .map(|pins| pins.values().copied().collect())
            .unwrap_or_default();
        let retained = self
            .retained_versions
            .lock()
            .map_err(|_| OntolithError::InvalidState("retained versions lock poisoned"))?;
        let mut candidates: Vec<u64> = retained
            .iter()
            .copied()
            .filter(|version| *version != latest && !pinned.contains(version))
            .collect();
        candidates.sort_unstable();

        let cf_v = self.cf(CF_VERSIONS)?;
        let cf_vq = self.cf(CF_VERSIONS_QUADS)?;
        let mut pruned = Vec::new();
        for version in candidates {
            if retained.len().saturating_sub(pruned.len()) <= keep {
                break;
            }
            let prefix = encode_u64(version);
            let doomed: Vec<Vec<u8>> = self
                .db
                .iterator_cf(cf_v, IteratorMode::From(&prefix, Direction::Forward))
                .filter_map(|item| {
                    item.ok()
                        .filter(|(k, _)| k.starts_with(&prefix))
                        .map(|(k, _)| k.to_vec())
                })
                .collect();
            for k in doomed {
                batch.delete_cf(cf_v, k);
            }
            let doomed: Vec<Vec<u8>> = self
                .db
                .iterator_cf(cf_vq, IteratorMode::From(&prefix, Direction::Forward))
                .filter_map(|item| {
                    item.ok()
                        .filter(|(k, _)| k.starts_with(&prefix))
                        .map(|(k, _)| k.to_vec())
                })
                .collect();
            for k in doomed {
                batch.delete_cf(cf_vq, k);
            }
            pruned.push(version);
        }
        Ok(pruned)
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

    /// Scan a column family decoding triples; `prefix` restricts to keys
    /// starting with the given bytes (index CF prefix lookups).
    fn scan_triples_with_prefix(
        &self,
        cf_name: &str,
        prefix: Option<&[u8]>,
    ) -> Result<Vec<Triple>, OntolithError> {
        let cf = self.cf(cf_name)?;
        let iter = match prefix {
            Some(p) => self
                .db
                .iterator_cf(cf, IteratorMode::From(p, Direction::Forward)),
            None => self.db.iterator_cf(cf, IteratorMode::Start),
        };
        let mut out = Vec::new();
        for item in iter {
            let (k, v) = item.map_err(rocks_err)?;
            if let Some(p) = prefix
                && !k.starts_with(p)
            {
                break;
            }
            out.push(decode_triple(&v)?);
        }
        Ok(out)
    }

    /// Scan the quads column family decoding quads, optionally restricted to a
    /// named-graph prefix.
    fn scan_quads_with_prefix(&self, prefix: Option<&[u8]>) -> Result<Vec<Quad>, OntolithError> {
        let cf = self.cf(CF_QUADS)?;
        let iter = match prefix {
            Some(p) => self
                .db
                .iterator_cf(cf, IteratorMode::From(p, Direction::Forward)),
            None => self.db.iterator_cf(cf, IteratorMode::Start),
        };
        let mut out = Vec::new();
        for item in iter {
            let (k, v) = item.map_err(rocks_err)?;
            if let Some(p) = prefix
                && !k.starts_with(p)
            {
                break;
            }
            out.push(decode_quad(&v)?);
        }
        Ok(out)
    }

    /// Committed triples/quads matching a subject (delete-by-key pre-image).
    fn scan_doomed_by_subject(
        &self,
        subject_id: NodeId,
    ) -> Result<(Vec<Triple>, Vec<Quad>), OntolithError> {
        let triples = self
            .scan_triples_with_prefix(CF_SPO_INDEX, Some(&encode_spo_subject_prefix(subject_id)))?;
        let quads = self
            .scan_quads_with_prefix(None)?
            .into_iter()
            .filter(|q| q.triple.subject == subject_id)
            .collect();
        Ok((triples, quads))
    }

    /// Clone of the staged operations of `txn_id` (empty when not staged).
    fn pending_ops(&self, txn_id: Option<TxnId>) -> Vec<WriteOperation> {
        txn_id
            .and_then(|id| {
                self.state
                    .read()
                    .ok()
                    .and_then(|s| s.pending_writes.get(&id).cloned())
            })
            .unwrap_or_default()
    }

    /// Apply operations to the durable batch. Maintains the primary CFs plus
    /// the SPO/POS/OSP index CFs (RFC-0001 §4). Returns the number of
    /// committed entries removed by `DeleteKey` operations.
    fn durable_apply_ops(
        &self,
        batch: &mut RocksBatch,
        operations: &[WriteOperation],
    ) -> Result<usize, OntolithError> {
        let cf_t = self.cf(CF_TRIPLES)?;
        let cf_q = self.cf(CF_QUADS)?;
        let cf_spo = self.cf(CF_SPO_INDEX)?;
        let cf_pos = self.cf(CF_POS_INDEX)?;
        let cf_osp = self.cf(CF_OSP_INDEX)?;
        let mut removed = 0usize;
        for op in operations {
            match op {
                WriteOperation::PutTriple(t) => {
                    let encoded = encode_triple(t);
                    batch.put_cf(cf_t, triple_key(t), &encoded);
                    batch.put_cf(
                        cf_spo,
                        encode_spo_key(t.subject, &t.predicate, &t.object),
                        &encoded,
                    );
                    batch.put_cf(
                        cf_pos,
                        encode_pos_key(&t.predicate, &t.object, t.subject),
                        &encoded,
                    );
                    batch.put_cf(
                        cf_osp,
                        encode_osp_key(&t.object, t.subject, &t.predicate),
                        &encoded,
                    );
                }
                WriteOperation::DeleteTriple(t) => {
                    batch.delete_cf(cf_t, triple_key(t));
                    batch.delete_cf(cf_spo, encode_spo_key(t.subject, &t.predicate, &t.object));
                    batch.delete_cf(cf_pos, encode_pos_key(&t.predicate, &t.object, t.subject));
                    batch.delete_cf(cf_osp, encode_osp_key(&t.object, t.subject, &t.predicate));
                }
                WriteOperation::PutQuad(q) => {
                    batch.put_cf(cf_q, quad_key(q), encode_quad(q));
                }
                WriteOperation::DeleteQuad(q) => {
                    batch.delete_cf(cf_q, quad_key(q));
                }
                WriteOperation::DeleteKey(key) => {
                    if let Some(subject_id) = key.components.first().copied() {
                        let (doomed, doomed_q) = self.scan_doomed_by_subject(subject_id)?;
                        removed += doomed.len() + doomed_q.len();
                        for t in doomed {
                            batch.delete_cf(cf_t, triple_key(&t));
                            batch.delete_cf(
                                cf_spo,
                                encode_spo_key(t.subject, &t.predicate, &t.object),
                            );
                            batch.delete_cf(
                                cf_pos,
                                encode_pos_key(&t.predicate, &t.object, t.subject),
                            );
                            batch.delete_cf(
                                cf_osp,
                                encode_osp_key(&t.object, t.subject, &t.predicate),
                            );
                        }
                        for q in doomed_q {
                            batch.delete_cf(cf_q, quad_key(&q));
                        }
                    }
                }
            }
        }
        Ok(removed)
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

        // Post-commit full state: drives the immutable MVCC snapshot below.
        let mut triples = self.scan_triples_with_prefix(CF_TRIPLES, None)?;
        let mut quads = self.scan_quads_with_prefix(None)?;
        Self::apply_committed_ops(&mut triples, &mut quads, &operations);

        let seq = self.next_version.fetch_add(1, Ordering::SeqCst);
        let mut rocks_batch = RocksBatch::default();
        self.durable_apply_ops(&mut rocks_batch, &operations)?;
        let cf_v = self.cf(CF_VERSIONS)?;
        let cf_vq = self.cf(CF_VERSIONS_QUADS)?;
        for triple in &triples {
            rocks_batch.put_cf(
                cf_v,
                Self::version_key(seq, &triple_key(triple)),
                encode_triple(triple),
            );
        }
        for quad in &quads {
            rocks_batch.put_cf(
                cf_vq,
                Self::version_key(seq, &quad_key(quad)),
                encode_quad(quad),
            );
        }
        rocks_batch.put_cf(self.cf(CF_META)?, META_NEXT_VERSION, encode_u64(seq + 1));
        self.append_wal_record(
            &mut rocks_batch,
            &WalRecord {
                txn_id,
                phase: WalPhase::Committed,
                operation_count: 0,
                operations: Vec::new(),
            },
        )?;
        let pruned = self.prune_locked(&mut rocks_batch, self.version_retention)?;
        self.db.write(rocks_batch).map_err(|e| {
            self.failed_commit_txn_count.fetch_add(1, Ordering::SeqCst);
            rocks_err(e)
        })?;

        let mut retained = self
            .retained_versions
            .lock()
            .map_err(|_| OntolithError::InvalidState("retained versions lock poisoned"))?;
        retained.insert(seq);
        for version in &pruned {
            retained.remove(version);
        }
        if !pruned.is_empty() {
            self.pruned_versions_count
                .fetch_add(pruned.len() as u64, Ordering::SeqCst);
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
        let subject_id = key.components[0];
        let (doomed_t, doomed_q) = self.scan_doomed_by_subject(subject_id)?;
        let removed = doomed_t.len() + doomed_q.len();
        if removed == 0 {
            return Ok(0);
        }
        let mut triples = self.scan_triples_with_prefix(CF_TRIPLES, None)?;
        let mut quads = self.scan_quads_with_prefix(None)?;
        triples.retain(|t| t.subject != subject_id);
        quads.retain(|q| q.triple.subject != subject_id);

        let seq = self.next_version.fetch_add(1, Ordering::SeqCst);
        let mut rocks_batch = RocksBatch::default();
        self.durable_apply_ops(
            &mut rocks_batch,
            std::slice::from_ref(&WriteOperation::DeleteKey(key.clone())),
        )?;
        let cf_v = self.cf(CF_VERSIONS)?;
        let cf_vq = self.cf(CF_VERSIONS_QUADS)?;
        for triple in &triples {
            rocks_batch.put_cf(
                cf_v,
                Self::version_key(seq, &triple_key(triple)),
                encode_triple(triple),
            );
        }
        for quad in &quads {
            rocks_batch.put_cf(
                cf_vq,
                Self::version_key(seq, &quad_key(quad)),
                encode_quad(quad),
            );
        }
        rocks_batch.put_cf(self.cf(CF_META)?, META_NEXT_VERSION, encode_u64(seq + 1));
        let pruned = self.prune_locked(&mut rocks_batch, self.version_retention)?;
        self.db.write(rocks_batch).map_err(rocks_err)?;

        let mut retained = self
            .retained_versions
            .lock()
            .map_err(|_| OntolithError::InvalidState("retained versions lock poisoned"))?;
        retained.insert(seq);
        for version in &pruned {
            retained.remove(version);
        }
        if !pruned.is_empty() {
            self.pruned_versions_count
                .fetch_add(pruned.len() as u64, Ordering::SeqCst);
        }
        Ok(removed)
    }

    fn snapshot_with(
        &self,
        consistency: ConsistencyLevel,
        read_txn_id: Option<TxnId>,
    ) -> SnapshotRef {
        let version = self.committed_version();
        let snapshot_id = self.next_snapshot_id.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut pins) = self.pinned_snapshots.lock() {
            pins.insert(snapshot_id, version);
        }
        SnapshotRef::new(snapshot_id, read_txn_id, consistency, version)
    }

    fn stats(&self) -> StorageStats {
        let triples = self
            .scan_triples_with_prefix(CF_TRIPLES, None)
            .unwrap_or_default();
        let quads = self.scan_quads_with_prefix(None).unwrap_or_default();
        let mut subjects = HashSet::new();
        let mut predicates = HashSet::new();
        let mut objects: Vec<Term> = Vec::new();
        let mut named_graphs = HashSet::new();
        for triple in &triples {
            subjects.insert(triple.subject);
            predicates.insert(triple.predicate.clone());
            if !objects.iter().any(|o| o == &triple.object) {
                objects.push(triple.object.clone());
            }
        }
        for quad in &quads {
            if let Some(g) = &quad.graph_name {
                named_graphs.insert(g.clone());
            }
        }
        let pending_transactions = self
            .state
            .read()
            .map(|s| s.pending_writes.len() as u64)
            .unwrap_or(0);
        StorageStats {
            triple_count: triples.len() as u64,
            quad_count: quads.len() as u64,
            distinct_subjects: subjects.len() as u64,
            distinct_predicates: predicates.len() as u64,
            distinct_objects: objects.len() as u64,
            named_graph_count: named_graphs.len() as u64,
            dictionary_entries: self.len() as u64,
            pending_transactions,
            wal_records: self.entries().len() as u64,
            index_kinds_active: 3,
            committed_versions: self.committed_version(),
            pruned_versions: self.pruned_versions_count.load(Ordering::SeqCst),
            pinned_snapshots: self.pinned_snapshot_count(),
        }
    }

    fn default_graph_triples_in_txn(&self, txn_id: Option<TxnId>) -> Vec<Triple> {
        let mut triples = self
            .scan_triples_with_prefix(CF_TRIPLES, None)
            .unwrap_or_default();
        let ops = self.pending_ops(txn_id);
        Self::apply_ops_to_triple_projection(&mut triples, &ops, None, None, None);
        triples
    }

    fn triples_by_subject_in_txn(&self, subject: NodeId, txn_id: Option<TxnId>) -> Vec<Triple> {
        let mut triples = self
            .scan_triples_with_prefix(CF_SPO_INDEX, Some(&encode_spo_subject_prefix(subject)))
            .unwrap_or_default();
        let ops = self.pending_ops(txn_id);
        Self::apply_ops_to_triple_projection(&mut triples, &ops, Some(subject), None, None);
        triples
    }

    fn triples_by_predicate_in_txn(&self, predicate: &Iri, txn_id: Option<TxnId>) -> Vec<Triple> {
        let mut triples = self
            .scan_triples_with_prefix(CF_POS_INDEX, Some(&encode_pos_predicate_prefix(predicate)))
            .unwrap_or_default();
        let ops = self.pending_ops(txn_id);
        Self::apply_ops_to_triple_projection(&mut triples, &ops, None, Some(predicate), None);
        triples
    }

    fn triples_by_object_in_txn(&self, object: &Term, txn_id: Option<TxnId>) -> Vec<Triple> {
        let mut triples = self
            .scan_triples_with_prefix(CF_OSP_INDEX, Some(&encode_osp_object_prefix(object)))
            .unwrap_or_default();
        let ops = self.pending_ops(txn_id);
        Self::apply_ops_to_triple_projection(&mut triples, &ops, None, None, Some(object));
        triples
    }

    fn named_graph_quads(&self) -> Vec<Quad> {
        self.scan_quads_with_prefix(None).unwrap_or_default()
    }

    fn quads_by_graph_in_txn(&self, graph_name: Option<&Iri>, txn_id: Option<TxnId>) -> Vec<Quad> {
        let mut quads = match graph_name {
            Some(g) => self
                .scan_quads_with_prefix(Some(&quad_graph_prefix(g)))
                .unwrap_or_default(),
            None => self
                .scan_triples_with_prefix(CF_TRIPLES, None)
                .unwrap_or_default()
                .into_iter()
                .map(Quad::in_default_graph)
                .collect(),
        };
        for op in &self.pending_ops(txn_id) {
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
        quads
    }

    fn quads_matching_in_graph(
        &self,
        graph_name: &Iri,
        subject: Option<NodeId>,
        predicate: Option<&Iri>,
        object: Option<&Term>,
        txn_id: Option<TxnId>,
    ) -> Vec<Quad> {
        let _ = txn_id;
        let mut quads = self
            .scan_quads_with_prefix(Some(&quad_graph_prefix(graph_name)))
            .unwrap_or_default();
        if let Some(s) = subject {
            quads.retain(|q| q.triple.subject == s);
        }
        if let Some(p) = predicate {
            quads.retain(|q| &q.triple.predicate == p);
        }
        if let Some(o) = object {
            quads.retain(|q| &q.triple.object == o);
        }
        quads
    }

    fn committed_version(&self) -> u64 {
        self.next_version.load(Ordering::SeqCst).saturating_sub(1)
    }

    fn version_count(&self) -> u64 {
        self.committed_version()
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
        let _commit = self
            .commit_lock
            .lock()
            .map_err(|_| OntolithError::InvalidState("commit lock poisoned"))?;
        let mut rocks_batch = RocksBatch::default();
        let pruned = self.prune_locked(&mut rocks_batch, retention)?;
        if !pruned.is_empty() {
            self.db.write(rocks_batch).map_err(rocks_err)?;
            let mut retained = self
                .retained_versions
                .lock()
                .map_err(|_| OntolithError::InvalidState("retained versions lock poisoned"))?;
            for version in &pruned {
                retained.remove(version);
            }
            self.pruned_versions_count
                .fetch_add(pruned.len() as u64, Ordering::SeqCst);
        }
        Ok(pruned.len())
    }

    fn triples_at_version_in_txn(&self, version: u64, txn_id: Option<TxnId>) -> Vec<Triple> {
        let retained = self
            .retained_versions
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        let mut triples = if version == 0 {
            Vec::new() // genesis: empty default graph
        } else if retained.contains(&version) {
            self.scan_version_triples(version).unwrap_or_default()
        } else if retained.first().is_none_or(|oldest| version >= *oldest) {
            // Newer than the oldest retained version: not pruned (either a
            // just-committed version or the latest) — serve the live state.
            self.scan_triples_with_prefix(CF_TRIPLES, None)
                .unwrap_or_default()
        } else {
            // Pruned: fall back to the oldest retained version.
            retained
                .first()
                .and_then(|oldest| self.scan_version_triples(*oldest).ok())
                .unwrap_or_default()
        };
        let ops = self.pending_ops(txn_id);
        Self::apply_ops_to_triple_projection(&mut triples, &ops, None, None, None);
        triples
    }

    fn quads_at_version(&self, version: u64) -> Vec<Quad> {
        let retained = self
            .retained_versions
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        if version == 0 {
            Vec::new()
        } else if retained.contains(&version) {
            self.scan_version_quads(version).unwrap_or_default()
        } else if retained.first().is_none_or(|oldest| version >= *oldest) {
            self.scan_quads_with_prefix(None).unwrap_or_default()
        } else {
            retained
                .first()
                .and_then(|oldest| self.scan_version_quads(*oldest).ok())
                .unwrap_or_default()
        }
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

    #[test]
    fn rocksdb_cf_index_scans_serve_bound_reads_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        {
            let engine = RocksDbStorageEngine::open(path).unwrap();
            let txn = TxnId::new(1);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![
                        WriteOperation::PutTriple(Triple {
                            subject: NodeId::new(1),
                            predicate: Iri::new("urn:p"),
                            object: Term::Iri(Iri::new("urn:o1")),
                        }),
                        WriteOperation::PutTriple(Triple {
                            subject: NodeId::new(1),
                            predicate: Iri::new("urn:q"),
                            object: Term::Iri(Iri::new("urn:o2")),
                        }),
                        WriteOperation::PutTriple(Triple {
                            subject: NodeId::new(2),
                            predicate: Iri::new("urn:p"),
                            object: Term::Iri(Iri::new("urn:o2")),
                        }),
                    ],
                })
                .unwrap();
            engine.commit_transaction(txn).unwrap();

            assert_eq!(
                engine.triples_by_subject_in_txn(NodeId::new(1), None).len(),
                2
            );
            assert_eq!(
                engine
                    .triples_by_predicate_in_txn(&Iri::new("urn:p"), None)
                    .len(),
                2
            );
            assert_eq!(
                engine
                    .triples_by_object_in_txn(&Term::Iri(Iri::new("urn:o2")), None)
                    .len(),
                2
            );
            assert_eq!(engine.default_graph_triples().len(), 3);

            let graph = Iri::new("urn:graph:cf");
            let quad_txn = TxnId::new(2);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: quad_txn,
                    operations: vec![WriteOperation::PutQuad(Quad::in_named_graph(
                        Triple::new(
                            NodeId::new(1),
                            Iri::new("urn:p"),
                            Term::Iri(Iri::new("urn:o1")),
                        ),
                        graph.clone(),
                    ))],
                })
                .unwrap();
            engine.commit_transaction(quad_txn).unwrap();
            assert_eq!(
                engine
                    .quads_matching_in_graph(&graph, Some(NodeId::new(1)), None, None, None)
                    .len(),
                1
            );
            assert_eq!(engine.quads_by_graph_in_txn(Some(&graph), None).len(), 1);
        }
        // Reopen without any in-memory index cache: CF scans serve reads.
        let engine = RocksDbStorageEngine::open(path).unwrap();
        assert_eq!(
            engine.triples_by_subject_in_txn(NodeId::new(1), None).len(),
            2
        );
        assert_eq!(
            engine
                .triples_by_predicate_in_txn(&Iri::new("urn:p"), None)
                .len(),
            2
        );
        assert_eq!(
            engine
                .triples_by_object_in_txn(&Term::Iri(Iri::new("urn:o2")), None)
                .len(),
            2
        );
        assert_eq!(engine.default_graph_triples().len(), 3);
        let graph = Iri::new("urn:graph:cf");
        assert_eq!(
            engine
                .quads_matching_in_graph(&graph, Some(NodeId::new(1)), None, None, None)
                .len(),
            1
        );
    }

    #[test]
    fn rocksdb_delete_by_key_clears_position_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        let txn = TxnId::new(7);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(5),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o1")),
                    }),
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(5),
                        predicate: Iri::new("urn:q"),
                        object: Term::Iri(Iri::new("urn:o2")),
                    }),
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(9),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o3")),
                    }),
                ],
            })
            .unwrap();
        engine.commit_transaction(txn).unwrap();

        let removed = engine
            .delete_by_key(&StorageKey::spo_subject(NodeId::new(5)))
            .unwrap();
        assert_eq!(removed, 2);
        assert!(
            engine
                .triples_by_subject_in_txn(NodeId::new(5), None)
                .is_empty()
        );
        assert_eq!(
            engine.triples_by_subject_in_txn(NodeId::new(9), None).len(),
            1
        );
        assert_eq!(
            engine
                .triples_by_predicate_in_txn(&Iri::new("urn:p"), None)
                .len(),
            1
        );
        assert_eq!(engine.default_graph_triples().len(), 1);

        drop(engine);
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        assert!(
            engine
                .triples_by_subject_in_txn(NodeId::new(5), None)
                .is_empty()
        );
        assert_eq!(engine.default_graph_triples().len(), 1);
    }

    #[test]
    fn rocksdb_permutation_keys_land_in_index_column_families() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        let t = Triple {
            subject: NodeId::new(3),
            predicate: Iri::new("urn:p"),
            object: Term::Iri(Iri::new("urn:o")),
        };
        let txn = TxnId::new(11);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![WriteOperation::PutTriple(t.clone())],
            })
            .unwrap();
        engine.commit_transaction(txn).unwrap();

        let spo = engine.cf(CF_SPO_INDEX).unwrap();
        assert!(
            engine
                .db
                .get_cf(spo, encode_spo_key(t.subject, &t.predicate, &t.object))
                .unwrap()
                .is_some()
        );
        let pos = engine.cf(CF_POS_INDEX).unwrap();
        assert!(
            engine
                .db
                .get_cf(pos, encode_pos_key(&t.predicate, &t.object, t.subject))
                .unwrap()
                .is_some()
        );
        let osp = engine.cf(CF_OSP_INDEX).unwrap();
        assert!(
            engine
                .db
                .get_cf(osp, encode_osp_key(&t.object, t.subject, &t.predicate))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn rocksdb_versioned_reads_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        {
            let engine = RocksDbStorageEngine::open(path).unwrap();
            let graph = Iri::new("urn:graph:mvcc:rocks");
            let first = TxnId::new(21);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: first,
                    operations: vec![
                        WriteOperation::PutTriple(Triple {
                            subject: NodeId::new(1),
                            predicate: Iri::new("urn:p"),
                            object: Term::Iri(Iri::new("urn:o1")),
                        }),
                        WriteOperation::PutQuad(Quad::in_named_graph(
                            Triple::new(
                                NodeId::new(1),
                                Iri::new("urn:p"),
                                Term::Iri(Iri::new("urn:o1")),
                            ),
                            graph.clone(),
                        )),
                    ],
                })
                .unwrap();
            engine.commit_transaction(first).unwrap();
            assert_eq!(engine.committed_version(), 1);

            let snapshot = engine.snapshot();
            assert_eq!(snapshot.version, 1);
            assert_eq!(engine.pinned_snapshot_count(), 1);

            let second = TxnId::new(22);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: second,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(2),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o2")),
                    })],
                })
                .unwrap();
            engine.commit_transaction(second).unwrap();
            assert_eq!(engine.committed_version(), 2);
            assert_eq!(engine.version_count(), 2);

            assert_eq!(engine.triples_at_version_in_txn(1, None).len(), 1);
            assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 2);
            assert_eq!(engine.quads_at_version(1).len(), 1);
            assert_eq!(engine.quads_at_version(2).len(), 1);
            assert!(engine.triples_at_version_in_txn(0, None).is_empty());

            engine.release_snapshot(snapshot.snapshot_id);
            assert_eq!(engine.pinned_snapshot_count(), 0);
        }
        // Reopen: version chain is durable, snapshots still isolated.
        let engine = RocksDbStorageEngine::open(path).unwrap();
        assert_eq!(engine.committed_version(), 2);
        assert_eq!(engine.version_count(), 2);
        assert_eq!(engine.triples_at_version_in_txn(1, None).len(), 1);
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 2);
        assert_eq!(engine.quads_at_version(1).len(), 1);
        assert_eq!(engine.default_graph_triples().len(), 2);
    }

    #[test]
    fn rocksdb_prune_preserves_pinned_older_version() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        for seq in 1..=2u64 {
            let txn = TxnId::new(200 + seq as u128);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(seq),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o")),
                    })],
                })
                .unwrap();
            engine.commit_transaction(txn).unwrap();
        }
        assert_eq!(engine.committed_version(), 2);

        // Pin version 2 before committing the remaining versions.
        let pinned = engine.snapshot_with(ConsistencyLevel::Strong, None);
        assert_eq!(pinned.version, 2);
        assert_eq!(engine.pinned_snapshot_count(), 1);
        for seq in 3..=5u64 {
            let txn = TxnId::new(200 + seq as u128);
            engine
                .apply_write_batch(&WriteBatch {
                    txn_id: txn,
                    operations: vec![WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(seq),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o")),
                    })],
                })
                .unwrap();
            engine.commit_transaction(txn).unwrap();
        }
        assert_eq!(engine.committed_version(), 5);

        // Prune with retention 1: keeps latest (5) + pinned (2); prunes 1,3,4.
        let pruned = engine.prune_versions(1).unwrap();
        assert_eq!(pruned, 3);
        assert_eq!(engine.pruned_version_count(), 3);
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 2);
        assert_eq!(engine.triples_at_version_in_txn(5, None).len(), 5);
        // Pruned version 1 falls back to the oldest retained version (2).
        assert_eq!(engine.triples_at_version_in_txn(1, None).len(), 2);

        engine.release_snapshot(pinned.snapshot_id);
        assert_eq!(engine.pinned_snapshot_count(), 0);
        let pruned = engine.prune_versions(1).unwrap();
        assert_eq!(pruned, 1); // version 2 is now prunable
        assert_eq!(engine.pruned_version_count(), 4);
        // Version 2 falls back to the oldest retained version (5).
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 5);

        drop(engine);
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        assert_eq!(engine.committed_version(), 5);
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 5);
        assert_eq!(engine.triples_at_version_in_txn(5, None).len(), 5);
    }

    #[test]
    fn rocksdb_delete_by_key_mints_version() {
        let dir = tempfile::tempdir().unwrap();
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        let txn = TxnId::new(301);
        engine
            .apply_write_batch(&WriteBatch {
                txn_id: txn,
                operations: vec![
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(5),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o1")),
                    }),
                    WriteOperation::PutTriple(Triple {
                        subject: NodeId::new(9),
                        predicate: Iri::new("urn:p"),
                        object: Term::Iri(Iri::new("urn:o2")),
                    }),
                ],
            })
            .unwrap();
        engine.commit_transaction(txn).unwrap();
        assert_eq!(engine.committed_version(), 1);

        let removed = engine
            .delete_by_key(&StorageKey::spo_subject(NodeId::new(5)))
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(engine.committed_version(), 2);
        assert_eq!(engine.triples_at_version_in_txn(1, None).len(), 2);
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 1);

        // Deleting a missing subject must not mint a version.
        let removed = engine
            .delete_by_key(&StorageKey::spo_subject(NodeId::new(99)))
            .unwrap();
        assert_eq!(removed, 0);
        assert_eq!(engine.committed_version(), 2);

        drop(engine);
        let engine = RocksDbStorageEngine::open(dir.path()).unwrap();
        assert_eq!(engine.committed_version(), 2);
        assert_eq!(engine.triples_at_version_in_txn(1, None).len(), 2);
        assert_eq!(engine.triples_at_version_in_txn(2, None).len(), 1);
    }
}
