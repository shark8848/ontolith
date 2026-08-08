//! RocksDB-backed openraft storage (L4, ADR-0004 decision 3).
//!
//! The dedicated `raft` column family of [`RocksDbStorageEngine`] holds the
//! openraft log entries, hard state (vote / committed / last applied /
//! purged), membership, the applied state-machine entries, and snapshot
//! bytes. All accesses go through the byte-level `raft_cf_*` primitives; the
//! key layout below is private to this module.

use super::{Entry, LogEntry, NodeId, StorageError, TypeConfig};
use crate::domain::{ClusterEpoch, LogPayload};
use ontolith_storage::infrastructure::RocksDbStorageEngine;
use openraft::storage::LogState;
use openraft::{
    AnyError, BasicNode, ErrorSubject, ErrorVerb, LogId, Snapshot, SnapshotMeta, StoredMembership,
    Vote,
};
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

/// Byte prefixes / fixed keys in the `raft` column family.
const PREFIX_LOG: &[u8] = b"log/";
const PREFIX_APPLIED: &[u8] = b"applied/";
const KEY_VOTE: &[u8] = b"vote";
const KEY_COMMITTED: &[u8] = b"committed";
const KEY_LAST_PURGED: &[u8] = b"last_purged";
const KEY_LAST_APPLIED: &[u8] = b"last_applied";
const KEY_LOG_LAST: &[u8] = b"log_last";
const KEY_MEMBERSHIP: &[u8] = b"membership";
const KEY_SNAPSHOT_META: &[u8] = b"snapshot_meta";
const KEY_SNAPSHOT_DATA: &[u8] = b"snapshot_data";

/// Build the byte key for `prefix + 8-byte big-endian index`.
fn index_key(prefix: &[u8], index: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 8);
    key.extend_from_slice(prefix);
    key.extend_from_slice(&index.to_be_bytes());
    key
}

/// One-past-the-end scan bound for a prefix (all keys sharing the prefix are
/// strictly less than `prefix ++ [0xFF]`).
fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    end.push(0xFF);
    end
}

/// Turn a log index range into `[from, to)` byte keys of the `log/` prefix.
fn scan_bounds<RB: RangeBounds<u64>>(range: &RB) -> (Vec<u8>, Vec<u8>) {
    use std::ops::Bound;
    let from = match range.start_bound() {
        Bound::Included(i) => index_key(PREFIX_LOG, *i),
        Bound::Excluded(i) => index_key(PREFIX_LOG, i.saturating_add(1)),
        Bound::Unbounded => PREFIX_LOG.to_vec(),
    };
    let to = match range.end_bound() {
        Bound::Included(i) => index_key(PREFIX_LOG, i.saturating_add(1)),
        Bound::Excluded(i) => index_key(PREFIX_LOG, *i),
        Bound::Unbounded => prefix_end(PREFIX_LOG),
    };
    (from, to)
}

fn storage_io(
    subject: &ErrorSubject<NodeId>,
    verb: ErrorVerb,
    err: impl std::error::Error + Send + Sync + 'static,
) -> StorageError {
    StorageError::IO {
        source: openraft::StorageIOError::new(subject.clone(), verb, AnyError::new(&err)),
    }
}

#[allow(clippy::result_large_err)] // openraft::StorageError is the mandated error type
fn encode_json<T: serde::Serialize>(
    subject: &ErrorSubject<NodeId>,
    verb: ErrorVerb,
    value: &T,
) -> Result<Vec<u8>, StorageError> {
    serde_json::to_vec(value).map_err(|e| storage_io(subject, verb, e))
}

#[allow(clippy::result_large_err)] // openraft::StorageError is the mandated error type
fn decode_json<T: serde::de::DeserializeOwned>(
    subject: &ErrorSubject<NodeId>,
    verb: ErrorVerb,
    bytes: &[u8],
) -> Result<T, StorageError> {
    serde_json::from_slice(bytes).map_err(|e| storage_io(subject, verb, e))
}

/// RocksDB-backed openraft storage (log + hard state + state machine).
#[derive(Clone)]
pub struct RocksRaftStorage {
    engine: Arc<RocksDbStorageEngine>,
}

impl RocksRaftStorage {
    pub fn new(engine: Arc<RocksDbStorageEngine>) -> Self {
        Self { engine }
    }

    /// Applied entries `[index, last_applied]` (feeds
    /// `Replicator::entries_from`).
    pub fn applied_entries_from(&self, index: u64) -> Vec<LogEntry> {
        let from = index_key(PREFIX_APPLIED, index);
        let to = prefix_end(PREFIX_APPLIED);
        match self.engine.raft_cf_scan_range(&from, &to) {
            Ok(raw) => raw
                .into_iter()
                .filter_map(|(_, v)| serde_json::from_slice(&v).ok())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[allow(clippy::result_large_err)] // openraft::StorageError is the mandated error type
    fn read_json_opt<T: serde::de::DeserializeOwned>(
        &self,
        key: &[u8],
        subject: &ErrorSubject<NodeId>,
    ) -> Result<Option<T>, StorageError> {
        match self
            .engine
            .raft_cf_get(key)
            .map_err(|e| storage_io(subject, ErrorVerb::Read, e))?
        {
            Some(bytes) => Ok(Some(decode_json(subject, ErrorVerb::Read, &bytes)?)),
            None => Ok(None),
        }
    }

    #[allow(clippy::result_large_err)] // openraft::StorageError is the mandated error type
    fn write_json<T: serde::Serialize>(
        &self,
        key: &[u8],
        value: &T,
        subject: &ErrorSubject<NodeId>,
    ) -> Result<(), StorageError> {
        let bytes = encode_json(subject, ErrorVerb::Write, value)?;
        self.engine
            .raft_cf_put(key, &bytes)
            .map_err(|e| storage_io(subject, ErrorVerb::Write, e))
    }
}

/// Clone-on-read log reader sharing the engine's `raft` CF.
#[derive(Clone)]
pub struct RocksLogReader {
    engine: Arc<RocksDbStorageEngine>,
}

impl openraft::RaftLogReader<TypeConfig> for RocksLogReader {
    async fn try_get_log_entries<RB>(&mut self, range: RB) -> Result<Vec<Entry>, StorageError>
    where
        RB: RangeBounds<u64> + Clone + std::fmt::Debug + openraft::OptionalSend,
    {
        let (from, to) = scan_bounds(&range);
        let raw = self
            .engine
            .raft_cf_scan_range(&from, &to)
            .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let mut out = Vec::new();
        for (_, value) in raw {
            out.push(decode_json(&ErrorSubject::Logs, ErrorVerb::Read, &value)?);
        }
        Ok(out)
    }
}

/// The storage type itself also reads the log (openraft 0.9 v1
/// `RaftStorage: RaftLogReader` supertrait); delegate to the same scan.
impl openraft::RaftLogReader<TypeConfig> for RocksRaftStorage {
    async fn try_get_log_entries<RB>(&mut self, range: RB) -> Result<Vec<Entry>, StorageError>
    where
        RB: RangeBounds<u64> + Clone + std::fmt::Debug + openraft::OptionalSend,
    {
        let (from, to) = scan_bounds(&range);
        let raw = self
            .engine
            .raft_cf_scan_range(&from, &to)
            .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Read, e))?;
        let mut out = Vec::new();
        for (_, value) in raw {
            out.push(decode_json(&ErrorSubject::Logs, ErrorVerb::Read, &value)?);
        }
        Ok(out)
    }
}

/// Snapshot builder reading the applied state from the `raft` CF.
#[derive(Clone)]
pub struct RocksSnapshotBuilder {
    engine: Arc<RocksDbStorageEngine>,
}

impl openraft::RaftSnapshotBuilder<TypeConfig> for RocksSnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError> {
        let raw = self
            .engine
            .raft_cf_scan_prefix(PREFIX_APPLIED)
            .map_err(|e| storage_io(&ErrorSubject::StateMachine, ErrorVerb::Read, e))?;
        let mut applied: Vec<LogEntry> = Vec::new();
        for (_, value) in raw {
            applied.push(decode_json(
                &ErrorSubject::StateMachine,
                ErrorVerb::Read,
                &value,
            )?);
        }

        let last_log_id: Option<LogId<NodeId>> = match self
            .engine
            .raft_cf_get(KEY_LAST_APPLIED)
            .map_err(|e| storage_io(&ErrorSubject::StateMachine, ErrorVerb::Read, e))?
        {
            Some(bytes) => Some(decode_json(
                &ErrorSubject::StateMachine,
                ErrorVerb::Read,
                &bytes,
            )?),
            None => None,
        };
        let last_membership: StoredMembership<NodeId, BasicNode> = match self
            .engine
            .raft_cf_get(KEY_MEMBERSHIP)
            .map_err(|e| storage_io(&ErrorSubject::StateMachine, ErrorVerb::Read, e))?
        {
            Some(bytes) => decode_json(&ErrorSubject::StateMachine, ErrorVerb::Read, &bytes)?,
            None => StoredMembership::default(),
        };

        let snapshot_id = match &last_log_id {
            Some(log_id) => format!("rocks-{}-{}", log_id.leader_id.term, log_id.index),
            None => "rocks-empty".to_owned(),
        };
        let meta = SnapshotMeta {
            last_log_id,
            last_membership,
            snapshot_id,
        };
        let data = encode_json(&ErrorSubject::Snapshot(None), ErrorVerb::Write, &applied)?;
        self.engine
            .raft_cf_put_batch(&[
                (
                    KEY_SNAPSHOT_META.to_vec(),
                    encode_json(&ErrorSubject::Snapshot(None), ErrorVerb::Write, &meta)?,
                ),
                (KEY_SNAPSHOT_DATA.to_vec(), data.clone()),
            ])
            .map_err(|e| storage_io(&ErrorSubject::Snapshot(None), ErrorVerb::Write, e))?;
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

impl openraft::RaftStorage<TypeConfig> for RocksRaftStorage {
    type LogReader = RocksLogReader;
    type SnapshotBuilder = RocksSnapshotBuilder;

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> Result<(), StorageError> {
        self.write_json(KEY_VOTE, vote, &ErrorSubject::Vote)
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<NodeId>>, StorageError> {
        self.read_json_opt(KEY_VOTE, &ErrorSubject::Vote)
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<NodeId>>,
    ) -> Result<(), StorageError> {
        match committed {
            Some(log_id) => self.write_json(KEY_COMMITTED, &log_id, &ErrorSubject::Store)?,
            None => self
                .engine
                .raft_cf_delete(KEY_COMMITTED)
                .map_err(|e| storage_io(&ErrorSubject::Store, ErrorVerb::Delete, e))?,
        }
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<NodeId>>, StorageError> {
        self.read_json_opt(KEY_COMMITTED, &ErrorSubject::Store)
    }

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError> {
        let last_purged: Option<LogId<NodeId>> =
            self.read_json_opt(KEY_LAST_PURGED, &ErrorSubject::Logs)?;
        let last_log_id: Option<LogId<NodeId>> =
            self.read_json_opt(KEY_LOG_LAST, &ErrorSubject::Logs)?;
        Ok(LogState {
            last_purged_log_id: last_purged,
            last_log_id: last_log_id.or(last_purged),
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        RocksLogReader {
            engine: self.engine.clone(),
        }
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError>
    where
        I: IntoIterator<Item = Entry> + openraft::OptionalSend,
    {
        let entries: Vec<Entry> = entries.into_iter().collect();
        if entries.is_empty() {
            return Ok(());
        }
        let mut batch = Vec::with_capacity(entries.len() + 1);
        for entry in &entries {
            batch.push((
                index_key(PREFIX_LOG, entry.log_id.index),
                encode_json(&ErrorSubject::Logs, ErrorVerb::Write, entry)?,
            ));
        }
        let last = entries.last().expect("non-empty entries").log_id;
        batch.push((
            KEY_LOG_LAST.to_vec(),
            encode_json(&ErrorSubject::Logs, ErrorVerb::Write, &last)?,
        ));
        self.engine
            .raft_cf_put_batch(&batch)
            .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Write, e))
    }

    async fn delete_conflict_logs_since(
        &mut self,
        log_id: LogId<NodeId>,
    ) -> Result<(), StorageError> {
        let from = index_key(PREFIX_LOG, log_id.index);
        let to = prefix_end(PREFIX_LOG);
        self.engine
            .raft_cf_delete_range(&from, &to)
            .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Delete, e))?;

        let last_purged: Option<LogId<NodeId>> =
            self.read_json_opt(KEY_LAST_PURGED, &ErrorSubject::Logs)?;
        let purged_index = last_purged.map(|l| l.index).unwrap_or(0);
        let new_last: Option<LogId<NodeId>> = if log_id.index > purged_index + 1 {
            let prev = self
                .engine
                .raft_cf_get(&index_key(PREFIX_LOG, log_id.index - 1))
                .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Read, e))?;
            match prev {
                Some(bytes) => {
                    let entry: Entry = decode_json(&ErrorSubject::Logs, ErrorVerb::Read, &bytes)?;
                    Some(entry.log_id)
                }
                None => last_purged,
            }
        } else {
            last_purged
        };
        match new_last {
            Some(last) => self.write_json(KEY_LOG_LAST, &last, &ErrorSubject::Logs)?,
            None => self
                .engine
                .raft_cf_delete(KEY_LOG_LAST)
                .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Delete, e))?,
        }
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<NodeId>) -> Result<(), StorageError> {
        let to = index_key(PREFIX_LOG, log_id.index.saturating_add(1));
        self.engine
            .raft_cf_delete_range(PREFIX_LOG, &to)
            .map_err(|e| storage_io(&ErrorSubject::Logs, ErrorVerb::Delete, e))?;
        self.write_json(KEY_LAST_PURGED, &log_id, &ErrorSubject::Logs)?;

        let current_last: Option<LogId<NodeId>> =
            self.read_json_opt(KEY_LOG_LAST, &ErrorSubject::Logs)?;
        let new_last = match current_last {
            Some(last) if last.index > log_id.index => last,
            _ => log_id,
        };
        self.write_json(KEY_LOG_LAST, &new_last, &ErrorSubject::Logs)
    }

    async fn last_applied_state(
        &mut self,
    ) -> Result<(Option<LogId<NodeId>>, StoredMembership<NodeId, BasicNode>), StorageError> {
        let last_applied: Option<LogId<NodeId>> =
            self.read_json_opt(KEY_LAST_APPLIED, &ErrorSubject::StateMachine)?;
        let membership: StoredMembership<NodeId, BasicNode> = match self
            .engine
            .raft_cf_get(KEY_MEMBERSHIP)
            .map_err(|e| storage_io(&ErrorSubject::StateMachine, ErrorVerb::Read, e))?
        {
            Some(bytes) => decode_json(&ErrorSubject::StateMachine, ErrorVerb::Read, &bytes)?,
            None => StoredMembership::default(),
        };
        Ok((last_applied, membership))
    }

    async fn apply_to_state_machine(
        &mut self,
        entries: &[Entry],
    ) -> Result<Vec<LogEntry>, StorageError> {
        let mut batch = Vec::new();
        let mut out = Vec::new();
        for entry in entries {
            let index = entry.log_id.index;
            let term = ClusterEpoch::new(entry.log_id.leader_id.term);
            let payload = match &entry.payload {
                openraft::EntryPayload::Normal(data) => data.clone(),
                openraft::EntryPayload::Blank => LogPayload::Noop,
                openraft::EntryPayload::Membership(membership) => {
                    let stored = StoredMembership::new(Some(entry.log_id), membership.clone());
                    batch.push((
                        KEY_MEMBERSHIP.to_vec(),
                        encode_json(&ErrorSubject::StateMachine, ErrorVerb::Write, &stored)?,
                    ));
                    LogPayload::Metadata(format!("membership:{membership:?}"))
                }
            };
            let applied = LogEntry {
                index,
                term,
                payload,
            };
            batch.push((
                index_key(PREFIX_APPLIED, index),
                encode_json(&ErrorSubject::StateMachine, ErrorVerb::Write, &applied)?,
            ));
            batch.push((
                KEY_LAST_APPLIED.to_vec(),
                encode_json(&ErrorSubject::StateMachine, ErrorVerb::Write, &entry.log_id)?,
            ));
            out.push(applied);
        }
        self.engine
            .raft_cf_put_batch(&batch)
            .map_err(|e| storage_io(&ErrorSubject::StateMachine, ErrorVerb::Write, e))?;
        Ok(out)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        RocksSnapshotBuilder {
            engine: self.engine.clone(),
        }
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Box<Cursor<Vec<u8>>>, StorageError> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError> {
        let bytes = snapshot.into_inner();
        let applied: Vec<LogEntry> =
            decode_json(&ErrorSubject::Snapshot(None), ErrorVerb::Read, &bytes)?;

        let mut ops = vec![ontolith_storage::infrastructure::RaftCfOp::DeleteRange(
            PREFIX_APPLIED.to_vec(),
            prefix_end(PREFIX_APPLIED),
        )];
        for entry in &applied {
            ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
                index_key(PREFIX_APPLIED, entry.index),
                encode_json(&ErrorSubject::Snapshot(None), ErrorVerb::Write, entry)?,
            ));
        }
        ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
            KEY_LAST_APPLIED.to_vec(),
            encode_json(
                &ErrorSubject::Snapshot(None),
                ErrorVerb::Write,
                &meta.last_log_id,
            )?,
        ));
        ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
            KEY_MEMBERSHIP.to_vec(),
            encode_json(
                &ErrorSubject::Snapshot(None),
                ErrorVerb::Write,
                &meta.last_membership,
            )?,
        ));
        if let Some(last_log_id) = meta.last_log_id {
            ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
                KEY_LOG_LAST.to_vec(),
                encode_json(
                    &ErrorSubject::Snapshot(None),
                    ErrorVerb::Write,
                    &last_log_id,
                )?,
            ));
            ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
                KEY_LAST_PURGED.to_vec(),
                encode_json(
                    &ErrorSubject::Snapshot(None),
                    ErrorVerb::Write,
                    &last_log_id,
                )?,
            ));
        }
        ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
            KEY_SNAPSHOT_META.to_vec(),
            encode_json(&ErrorSubject::Snapshot(None), ErrorVerb::Write, meta)?,
        ));
        ops.push(ontolith_storage::infrastructure::RaftCfOp::Put(
            KEY_SNAPSHOT_DATA.to_vec(),
            bytes,
        ));

        // Atomically replace the applied state machine + snapshot refs.
        self.engine
            .raft_cf_write_batch(&ops)
            .map_err(|e| storage_io(&ErrorSubject::Snapshot(None), ErrorVerb::Write, e))
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<TypeConfig>>, StorageError> {
        let meta: Option<SnapshotMeta<NodeId, BasicNode>> =
            self.read_json_opt(KEY_SNAPSHOT_META, &ErrorSubject::Snapshot(None))?;
        let Some(meta) = meta else {
            return Ok(None);
        };
        let data = self
            .engine
            .raft_cf_get(KEY_SNAPSHOT_DATA)
            .map_err(|e| storage_io(&ErrorSubject::Snapshot(None), ErrorVerb::Read, e))?
            .unwrap_or_default();
        Ok(Some(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LogPayload, ShardId};
    use openraft::storage::{RaftLogReader, RaftSnapshotBuilder, RaftStorage};

    fn entry(index: u64, term: u64, payload: LogPayload) -> Entry {
        openraft::Entry {
            log_id: openraft::LogId {
                leader_id: openraft::LeaderId::new(term, 0),
                index,
            },
            payload: openraft::EntryPayload::Normal(payload),
        }
    }

    fn log_id(term: u64, node_id: u64, index: u64) -> openraft::LogId<NodeId> {
        openraft::LogId {
            leader_id: openraft::LeaderId::new(term, node_id),
            index,
        }
    }

    fn open_store(dir: &std::path::Path) -> RocksRaftStorage {
        RocksRaftStorage::new(Arc::new(
            RocksDbStorageEngine::open(dir.join("raft-db")).expect("open raft db"),
        ))
    }

    #[tokio::test]
    async fn rocksdb_log_append_read_purge_delete_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open_store(dir.path());

        let e1 = entry(1, 1, LogPayload::Metadata("alpha".into()));
        let e2 = entry(
            2,
            1,
            LogPayload::Data {
                shard_id: ShardId::new(3),
                op: "write".into(),
            },
        );
        store
            .append_to_log(vec![e1.clone(), e2.clone()])
            .await
            .unwrap();

        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_purged_log_id, None);
        assert_eq!(state.last_log_id, Some(e2.log_id));

        let entries = store.try_get_log_entries(1..3).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].log_id.index, 1);
        assert_eq!(entries[1].log_id.index, 2);

        // purge up to index 1
        store.purge_logs_upto(log_id(1, 0, 1)).await.unwrap();
        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_purged_log_id, Some(log_id(1, 0, 1)));
        assert_eq!(state.last_log_id, Some(e2.log_id));

        // delete conflicting entries since index 2 (keeps none)
        store
            .delete_conflict_logs_since(log_id(1, 0, 2))
            .await
            .unwrap();
        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_log_id, Some(log_id(1, 0, 1)));

        // append after delete re-bases the last log id
        let e3 = entry(2, 2, LogPayload::Metadata("beta".into()));
        store.append_to_log(vec![e3.clone()]).await.unwrap();
        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_log_id, Some(e3.log_id));
    }

    #[tokio::test]
    async fn rocksdb_snapshot_build_and_install_roundtrip() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let mut store_a = open_store(dir_a.path());
        let mut store_b = open_store(dir_b.path());

        let e1 = entry(1, 1, LogPayload::Metadata("alpha".into()));
        let e2 = entry(
            2,
            1,
            LogPayload::Data {
                shard_id: ShardId::new(7),
                op: "write".into(),
            },
        );
        store_a
            .append_to_log(vec![e1.clone(), e2.clone()])
            .await
            .unwrap();
        let applied = store_a
            .apply_to_state_machine(&[e1, e2.clone()])
            .await
            .unwrap();
        assert_eq!(applied.len(), 2);
        let (last_applied, _membership) = store_a.last_applied_state().await.unwrap();
        assert_eq!(last_applied, Some(e2.log_id));

        // Build + persist snapshot on the leader store.
        let mut builder = store_a.get_snapshot_builder().await;
        let snapshot = builder.build_snapshot().await.unwrap();
        assert_eq!(snapshot.meta.last_log_id, Some(e2.log_id));
        assert!(!snapshot.snapshot.get_ref().is_empty());

        let current = store_a
            .get_current_snapshot()
            .await
            .unwrap()
            .expect("current snapshot");
        assert_eq!(current.meta.snapshot_id, snapshot.meta.snapshot_id);

        // Install the snapshot bytes on the follower store.
        let mut incoming = store_b.begin_receiving_snapshot().await.unwrap();
        std::io::Write::write_all(&mut incoming, snapshot.snapshot.get_ref()).unwrap();
        store_b
            .install_snapshot(&snapshot.meta, incoming)
            .await
            .unwrap();

        let (last_applied, _) = store_b.last_applied_state().await.unwrap();
        assert_eq!(last_applied, Some(e2.log_id));
        let entries = store_b.applied_entries_from(1);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].payload, LogPayload::Metadata("alpha".into()));
        assert_eq!(
            entries[1].payload,
            LogPayload::Data {
                shard_id: ShardId::new(7),
                op: "write".into(),
            }
        );
        let current_b = store_b
            .get_current_snapshot()
            .await
            .unwrap()
            .expect("installed snapshot");
        assert_eq!(current_b.meta.last_log_id, Some(e2.log_id));
    }
}
