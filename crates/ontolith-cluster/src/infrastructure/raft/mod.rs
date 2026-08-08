//! Raft data plane backend (L4, ADR-0004).
//!
//! M1: openraft single-node bootstrap + cluster trait adapter + in-memory
//! transport; M2: multi-process HTTP RPC (in-tree HTTP/1.1 + shared secret)
//! and RocksDB `raft` CF storage (see [`rocks_store`]). The consensus-
//! relevant contracts (leadership, metadata epoch, replication log) are
//! backed by an [`openraft::Raft`] node; the control-plane utilities (shard
//! routing, rebalance, data-plane sync, fault injection) delegate to the
//! in-process [`InMemoryClusterRuntime`] simulator until M3.
//!
//! [`InMemoryClusterRuntime`]: super::InMemoryClusterRuntime

use crate::application::{
    ClusterRuntime, DataPlaneSync, ElectionService, FailoverController, FaultInjector,
    MetadataService, RebalanceService, Replicator, ShardRouter,
};
use crate::domain::{
    ClusterEpoch, ClusterNode, ClusterNodeId, ClusterStatus, FailoverEvent, LogEntry, LogPayload,
    Membership, NetworkPartition, RebalancePlan, SessionId, ShardId, ShardMap, SlotRange,
    SyncReceipt,
};
use crate::infrastructure::{ClusterConfig, InMemoryClusterRuntime};
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;
use ontolith_storage::domain::SnapshotRef;
use ontolith_storage::infrastructure::RocksDbStorageEngine;
use openraft::storage::Adaptor;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::ops::RangeBounds;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

mod rocks_store;
pub use rocks_store::RocksRaftStorage;

mod http;
pub use http::{HttpRaftClient, HttpRaftFactory, HttpRaftServer};

/// Openraft type configuration for the cluster data plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct TypeConfig;

impl openraft::RaftTypeConfig for TypeConfig {
    type D = LogPayload;
    type R = LogEntry;
    type NodeId = u64;
    type Node = openraft::BasicNode;
    type Entry = openraft::Entry<TypeConfig>;
    type SnapshotData = Cursor<Vec<u8>>;
    type AsyncRuntime = openraft::TokioRuntime;
    type Responder = openraft::raft::responder::OneshotResponder<TypeConfig>;
}

type NodeId = u64;
type Entry = openraft::Entry<TypeConfig>;
type StorageError = openraft::StorageError<NodeId>;

/// Configuration for the raft-backed cluster runtime.
#[derive(Debug, Clone)]
pub struct RaftClusterConfig {
    /// Stable openraft node id.
    pub node_id: u64,
    pub region: String,
    pub slot_count: u32,
    pub shard_count: u32,
    pub max_eventual_lag: u64,
    /// Raft heartbeat interval in milliseconds (tuned down in tests).
    pub heartbeat_interval_ms: u64,
    /// Optional HTTP listen address for the raft RPC server (M2, ADR-0004);
    /// e.g. `127.0.0.1:0` binds a free port. `None` keeps the in-memory
    /// transport (M1 test harness).
    pub http_listen_addr: Option<String>,
    /// Shared cluster secret for raft RPC peer auth (M2). Must be non-empty
    /// when HTTP transport is enabled.
    pub raft_secret: String,
    /// Optional RocksDB storage path for the raft log/state/snapshot (M2,
    /// ADR-0004 decision 3); `None` keeps the in-memory storage fallback.
    pub raft_storage_path: Option<PathBuf>,
}

impl Default for RaftClusterConfig {
    fn default() -> Self {
        Self {
            node_id: 0,
            region: "default".into(),
            slot_count: 1024,
            shard_count: 2,
            max_eventual_lag: 100,
            heartbeat_interval_ms: 200,
            http_listen_addr: None,
            raft_secret: String::new(),
            raft_storage_path: None,
        }
    }
}

/// Process-wide registry mapping raft node ids to live [`openraft::Raft`]
/// handles. The in-memory transport (M1) and test harness; M2 adds the HTTP
/// transport ([`HttpRaftFactory`]) for cross-process peers.
#[derive(Clone, Default)]
pub struct RaftRegistry {
    inner: Arc<Mutex<HashMap<NodeId, Arc<openraft::Raft<TypeConfig>>>>>,
}

impl RaftRegistry {
    pub fn register(&self, id: NodeId, raft: Arc<openraft::Raft<TypeConfig>>) {
        self.inner.lock().unwrap().insert(id, raft);
    }

    pub fn get(&self, id: NodeId) -> Option<Arc<openraft::Raft<TypeConfig>>> {
        self.inner.lock().unwrap().get(&id).cloned()
    }

    pub fn ids(&self) -> Vec<NodeId> {
        self.inner.lock().unwrap().keys().copied().collect()
    }
}

// ---------------------------------------------------------------------------
// In-memory storage (log + state machine; test fallback alongside the M2
// RocksDB `raft` CF storage in `rocks_store`).
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct MemInner {
    entries: BTreeMap<u64, Entry>,
    vote: Option<openraft::Vote<NodeId>>,
    committed: Option<openraft::LogId<NodeId>>,
    last_purged: Option<openraft::LogId<NodeId>>,
    last_applied: Option<openraft::LogId<NodeId>>,
    membership: openraft::StoredMembership<NodeId, openraft::BasicNode>,
    applied: BTreeMap<u64, LogEntry>,
}

#[derive(Clone, Default)]
pub struct MemStorage {
    inner: Arc<RwLock<MemInner>>,
}

impl MemStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applied entries `[index, last_applied]` (feeds `Replicator::entries_from`).
    pub fn applied_entries_from(&self, index: u64) -> Vec<LogEntry> {
        let inner = self.inner.read().unwrap();
        inner
            .applied
            .range(index..)
            .map(|(_, e)| e.clone())
            .collect()
    }

}

/// Clone-on-read log reader sharing the store's backing map.
#[derive(Clone, Default)]
pub struct MemLogReader {
    inner: Arc<RwLock<MemInner>>,
}

impl openraft::RaftLogReader<TypeConfig> for MemStorage {
    async fn try_get_log_entries<RB>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry>, StorageError>
    where
        RB: RangeBounds<u64> + Clone + std::fmt::Debug + openraft::OptionalSend,
    {
        let inner = self.inner.read().unwrap();
        Ok(inner.entries.range(range).map(|(_, e)| e.clone()).collect())
    }
}

impl openraft::RaftLogReader<TypeConfig> for MemLogReader {
    async fn try_get_log_entries<RB>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry>, StorageError>
    where
        RB: RangeBounds<u64> + Clone + std::fmt::Debug + openraft::OptionalSend,
    {
        let inner = self.inner.read().unwrap();
        Ok(inner.entries.range(range).map(|(_, e)| e.clone()).collect())
    }
}

#[derive(Clone, Default)]
pub struct MemSnapshotBuilder {
    inner: Arc<RwLock<MemInner>>,
}

impl openraft::RaftSnapshotBuilder<TypeConfig> for MemSnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<openraft::Snapshot<TypeConfig>, StorageError> {
        let inner = self.inner.read().unwrap();
        let last_log_id = inner.last_applied;
        let last_membership = inner.membership.clone();
        let snapshot_id = match &last_log_id {
            Some(l) => format!("mem-{}-{}", l.leader_id.term, l.index),
            None => "mem-empty".to_string(),
        };
        Ok(openraft::Snapshot {
            meta: openraft::SnapshotMeta {
                last_log_id,
                last_membership,
                snapshot_id,
            },
            snapshot: Box::new(Cursor::new(Vec::new())),
        })
    }
}

impl openraft::RaftStorage<TypeConfig> for MemStorage {
    type LogReader = MemLogReader;
    type SnapshotBuilder = MemSnapshotBuilder;

    async fn save_vote(&mut self, vote: &openraft::Vote<NodeId>) -> Result<(), StorageError> {
        self.inner.write().unwrap().vote = Some(*vote);
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<openraft::Vote<NodeId>>, StorageError> {
        Ok(self.inner.read().unwrap().vote)
    }

    async fn save_committed(
        &mut self,
        committed: Option<openraft::LogId<NodeId>>,
    ) -> Result<(), StorageError> {
        self.inner.write().unwrap().committed = committed;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<openraft::LogId<NodeId>>, StorageError> {
        Ok(self.inner.read().unwrap().committed)
    }

    async fn get_log_state(&mut self) -> Result<openraft::LogState<TypeConfig>, StorageError> {
        let inner = self.inner.read().unwrap();
        let last_log_id = inner
            .entries
            .iter()
            .next_back()
            .map(|(_, e)| e.log_id)
            .or_else(|| inner.last_purged);
        Ok(openraft::LogState {
            last_purged_log_id: inner.last_purged,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        MemLogReader {
            inner: self.inner.clone(),
        }
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError>
    where
        I: IntoIterator<Item = Entry> + openraft::OptionalSend,
    {
        let mut inner = self.inner.write().unwrap();
        for entry in entries {
            inner.entries.insert(entry.log_id.index, entry);
        }
        Ok(())
    }

    async fn delete_conflict_logs_since(&mut self, log_id: openraft::LogId<NodeId>) -> Result<(), StorageError> {
        let mut inner = self.inner.write().unwrap();
        inner.entries.retain(|index, _| *index < log_id.index);
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: openraft::LogId<NodeId>) -> Result<(), StorageError> {
        let mut inner = self.inner.write().unwrap();
        inner.entries.retain(|index, _| *index > log_id.index);
        let cur = inner.last_purged.unwrap_or_default();
        if log_id.index > cur.index {
            inner.last_purged = Some(log_id);
        }
        Ok(())
    }

    async fn last_applied_state(
        &mut self,
    ) -> Result<
        (
            Option<openraft::LogId<NodeId>>,
            openraft::StoredMembership<NodeId, openraft::BasicNode>,
        ),
        StorageError,
    > {
        let inner = self.inner.read().unwrap();
        Ok((inner.last_applied, inner.membership.clone()))
    }

    async fn apply_to_state_machine(&mut self, entries: &[Entry]) -> Result<Vec<LogEntry>, StorageError> {
        let mut inner = self.inner.write().unwrap();
        let mut out = Vec::new();
        for entry in entries {
            let index = entry.log_id.index;
            let term = ClusterEpoch::new(entry.log_id.leader_id.term);
            let payload = match &entry.payload {
                openraft::EntryPayload::Normal(d) => d.clone(),
                openraft::EntryPayload::Blank => LogPayload::Noop,
                openraft::EntryPayload::Membership(m) => {
                    inner.membership =
                        openraft::StoredMembership::new(Some(entry.log_id), m.clone());
                    LogPayload::Metadata(format!("membership:{m:?}"))
                }
            };
            let applied = LogEntry {
                index,
                term,
                payload,
            };
            inner.applied.insert(index, applied.clone());
            inner.last_applied = Some(entry.log_id);
            out.push(applied);
        }
        Ok(out)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        MemSnapshotBuilder {
            inner: self.inner.clone(),
        }
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Box<Cursor<Vec<u8>>>, StorageError> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &openraft::SnapshotMeta<NodeId, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError> {
        let mut inner = self.inner.write().unwrap();
        inner.last_applied = meta.last_log_id;
        inner.membership = meta.last_membership.clone();
        drop(snapshot);
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<openraft::Snapshot<TypeConfig>>, StorageError> {
        let inner = self.inner.read().unwrap();
        Ok(match &inner.last_applied {
            Some(last_log_id) => {
                let meta = openraft::SnapshotMeta {
                    last_log_id: Some(*last_log_id),
                    last_membership: inner.membership.clone(),
                    snapshot_id: format!("mem-{}-{}", last_log_id.leader_id.term, last_log_id.index),
                };
                Some(openraft::Snapshot {
                    meta,
                    snapshot: Box::new(Cursor::new(Vec::new())),
                })
            }
            None => None,
        })
    }
}

// ---------------------------------------------------------------------------
// In-memory transport (M1; M2 adds the HTTP RPC client in `http`).
// ---------------------------------------------------------------------------

/// In-memory [`openraft::RaftNetworkFactory`]: routes RPCs to live raft
/// handles in [`RaftRegistry`].
#[derive(Clone)]
pub struct MemNetworkFactory {
    registry: Arc<RaftRegistry>,
}

impl MemNetworkFactory {
    pub fn new(registry: Arc<RaftRegistry>) -> Self {
        Self { registry }
    }
}

impl openraft::RaftNetworkFactory<TypeConfig> for MemNetworkFactory {
    type Network = MemNetwork;

    async fn new_client(&mut self, target: NodeId, _node: &openraft::BasicNode) -> Self::Network {
        MemNetwork {
            registry: self.registry.clone(),
            target,
        }
    }
}

/// In-memory network connection to one raft node.
#[derive(Clone)]
pub struct MemNetwork {
    registry: Arc<RaftRegistry>,
    target: NodeId,
}

type NetError = openraft::error::RPCError<NodeId, openraft::BasicNode, openraft::error::RaftError<NodeId>>;

impl MemNetwork {
    #[allow(clippy::result_large_err)] // error type is mandated by openraft::RaftNetwork
    fn target_raft(&self) -> Result<Arc<openraft::Raft<TypeConfig>>, NetError> {
        self.registry.get(self.target).ok_or_else(|| {
            openraft::error::RPCError::Unreachable(openraft::error::Unreachable::new(
                &std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no raft node registered for id {}", self.target),
                ),
            ))
        })
    }
}

impl openraft::RaftNetwork<TypeConfig> for MemNetwork {
    async fn append_entries(
        &mut self,
        rpc: openraft::raft::AppendEntriesRequest<TypeConfig>,
        _option: openraft::network::RPCOption,
    ) -> Result<openraft::raft::AppendEntriesResponse<NodeId>, NetError> {
        let raft = self.target_raft()?;
        raft.append_entries(rpc)
            .await
            .map_err(|e| openraft::error::RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }

    async fn install_snapshot(
        &mut self,
        rpc: openraft::raft::InstallSnapshotRequest<TypeConfig>,
        _option: openraft::network::RPCOption,
    ) -> Result<
        openraft::raft::InstallSnapshotResponse<NodeId>,
        openraft::error::RPCError<
            NodeId,
            openraft::BasicNode,
            openraft::error::RaftError<NodeId, openraft::error::InstallSnapshotError>,
        >,
    > {
        let raft = self.target_raft().map_err(|e| match e {
            openraft::error::RPCError::Unreachable(u) => openraft::error::RPCError::Unreachable(u),
            _ => openraft::error::RPCError::Network(openraft::error::NetworkError::new(
                &std::io::Error::other("snapshot rpc failed"),
            )),
        })?;
        raft.install_snapshot(rpc)
            .await
            .map_err(|e| openraft::error::RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }

    async fn vote(
        &mut self,
        rpc: openraft::raft::VoteRequest<NodeId>,
        _option: openraft::network::RPCOption,
    ) -> Result<openraft::raft::VoteResponse<NodeId>, NetError> {
        let raft = self.target_raft()?;
        raft.vote(rpc)
            .await
            .map_err(|e| openraft::error::RPCError::RemoteError(openraft::error::RemoteError::new(self.target, e)))
    }
}

// ---------------------------------------------------------------------------
// Raft-backed cluster runtime (trait adapter).
// ---------------------------------------------------------------------------
// Storage / transport selection (M2).
// ---------------------------------------------------------------------------

/// Storage backend behind the raft node (M2: RocksDB `raft` CF or memory).
enum RaftStoreKind {
    Mem(MemStorage),
    Rocks(RocksRaftStorage),
}

impl RaftStoreKind {
    fn applied_entries_from(&self, index: u64) -> Vec<LogEntry> {
        match self {
            Self::Mem(store) => store.applied_entries_from(index),
            Self::Rocks(store) => store.applied_entries_from(index),
        }
    }
}

/// Transport factory behind the raft node (M2: HTTP RPC or in-memory).
enum RaftNetKind {
    Mem(MemNetworkFactory),
    Http(HttpRaftFactory),
}

/// Build an openraft node over the v1 [`openraft::RaftStorage`] adapter.
fn build_node<S, N>(
    rt: &Arc<tokio::runtime::Runtime>,
    node_id: u64,
    raft_config: Arc<openraft::Config>,
    storage: S,
    network: N,
) -> Arc<openraft::Raft<TypeConfig>>
where
    S: openraft::RaftStorage<TypeConfig> + Send + Sync + 'static,
    N: openraft::RaftNetworkFactory<TypeConfig> + Send + Sync + 'static,
{
    let (log_store, state_machine) = Adaptor::new(storage);
    let raft = rt
        .block_on(openraft::Raft::new(
            node_id,
            raft_config,
            network,
            log_store,
            state_machine,
        ))
        .expect("start raft node");
    Arc::new(raft)
}

// ---------------------------------------------------------------------------

/// L4 cluster runtime backed by an [`openraft::Raft`] node.
///
/// Consensus contracts (leadership, epoch, replication log, committed
/// index) come from openraft; shard routing and the remaining control-plane
/// utilities delegate to the in-process simulator until M3.
pub struct RaftClusterRuntime {
    config: RaftClusterConfig,
    rt: Arc<tokio::runtime::Runtime>,
    raft: Arc<openraft::Raft<TypeConfig>>,
    store: RaftStoreKind,
    /// cluster node id string -> raft node id
    raft_ids: RwLock<HashMap<String, u64>>,
    /// raft node id -> cluster node id string
    cluster_ids: RwLock<HashMap<u64, String>>,
    bootstrapped: AtomicBool,
    inner: InMemoryClusterRuntime,
    /// M2: raft RPC server owned by this node (stopped on drop).
    http_server: Option<HttpRaftServer>,
    /// Leader-perspective per-follower replicated (acked) index watermark.
    /// Lets `replicate_to_followers` report how many entries were newly
    /// acked since the previous call (M3 real replication semantics).
    replicated_watermark: Mutex<HashMap<u64, u64>>,
}

impl RaftClusterRuntime {
    /// Build a runtime with a fresh process-wide registry and the given
    /// raft node id.
    pub fn new(config: RaftClusterConfig) -> Self {
        Self::new_with_registry(config, Arc::new(RaftRegistry::default()))
    }

    pub fn with_defaults() -> Self {
        Self::new(RaftClusterConfig::default())
    }

    /// Build a runtime sharing a [`RaftRegistry`] (in-process multi-node
    /// test harness for the M1 in-memory transport).
    #[allow(clippy::field_reassign_with_default)] // openraft::Config has no struct-update constructor
    pub fn new_with_registry(config: RaftClusterConfig, registry: Arc<RaftRegistry>) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime"),
        );
        let mut raft_config = openraft::Config::default();
        raft_config.cluster_name = format!("ontolith-{}", config.region);
        raft_config.heartbeat_interval = config.heartbeat_interval_ms;
        raft_config.election_timeout_min = config.heartbeat_interval_ms * 3;
        raft_config.election_timeout_max = config.heartbeat_interval_ms * 6;
        raft_config.enable_tick = true;
        let raft_config = Arc::new(raft_config);

        let store = match &config.raft_storage_path {
            Some(path) => {
                let engine = Arc::new(
                    RocksDbStorageEngine::open(path)
                        .expect("open raft rocksdb storage (raft_storage_path)"),
                );
                RaftStoreKind::Rocks(RocksRaftStorage::new(engine))
            }
            None => RaftStoreKind::Mem(MemStorage::new()),
        };
        let net = match &config.http_listen_addr {
            Some(_) => RaftNetKind::Http(HttpRaftFactory::new(config.raft_secret.clone())),
            None => RaftNetKind::Mem(MemNetworkFactory::new(registry.clone())),
        };

        let node_id = config.node_id;
        let raft = match (&store, &net) {
            (RaftStoreKind::Mem(store), RaftNetKind::Mem(net)) => {
                build_node(&rt, node_id, raft_config.clone(), store.clone(), net.clone())
            }
            (RaftStoreKind::Mem(store), RaftNetKind::Http(net)) => {
                build_node(&rt, node_id, raft_config.clone(), store.clone(), net.clone())
            }
            (RaftStoreKind::Rocks(store), RaftNetKind::Mem(net)) => {
                build_node(&rt, node_id, raft_config.clone(), store.clone(), net.clone())
            }
            (RaftStoreKind::Rocks(store), RaftNetKind::Http(net)) => {
                build_node(&rt, node_id, raft_config.clone(), store.clone(), net.clone())
            }
        };
        if let RaftNetKind::Mem(_) = &net {
            registry.register(node_id, raft.clone());
        }

        let http_server = match (&config.http_listen_addr, &net) {
            (Some(listen), RaftNetKind::Http(_)) => {
                if config.raft_secret.is_empty() {
                    panic!("raft HTTP transport requires a non-empty raft_secret");
                }
                let listen: std::net::SocketAddr = listen
                    .parse()
                    .expect("invalid raft http listen address (http_listen_addr)");
                Some(
                    HttpRaftServer::spawn(
                        listen,
                        config.raft_secret.clone(),
                        Arc::clone(&rt),
                        Arc::clone(&raft),
                    )
                    .expect("bind raft http server"),
                )
            }
            _ => None,
        };

        let inner = InMemoryClusterRuntime::new(ClusterConfig {
            region: config.region.clone(),
            slot_count: config.slot_count,
            shard_count: config.shard_count,
            ..ClusterConfig::default()
        });

        Self {
            config,
            rt,
            raft,
            store,
            raft_ids: RwLock::new(HashMap::new()),
            cluster_ids: RwLock::new(HashMap::new()),
            bootstrapped: AtomicBool::new(false),
            inner,
            http_server,
            replicated_watermark: Mutex::new(HashMap::new()),
        }
    }

    pub fn node_id(&self) -> u64 {
        self.config.node_id
    }

    /// The actually-bound HTTP raft RPC address (M2), when HTTP transport is
    /// enabled.
    pub fn http_addr(&self) -> Option<std::net::SocketAddr> {
        self.http_server.as_ref().map(|server| server.addr)
    }

    /// Map a cluster node id string to its raft id; assigns the next free
    /// raft id on first sight.
    fn raft_id_for(&self, cluster_id: &ClusterNodeId) -> u64 {
        let mut raft_ids = self.raft_ids.write().unwrap();
        if let Some(id) = raft_ids.get(cluster_id.as_str()) {
            return *id;
        }
        let next = raft_ids.len() as u64;
        raft_ids.insert(cluster_id.as_str().to_owned(), next);
        self.cluster_ids
            .write()
            .unwrap()
            .insert(next, cluster_id.as_str().to_owned());
        next
    }

    fn cluster_id_for(&self, raft_id: u64) -> ClusterNodeId {
        self.cluster_ids
            .read()
            .unwrap()
            .get(&raft_id)
            .cloned()
            .map(ClusterNodeId::new)
            .unwrap_or_else(|| ClusterNodeId::new(format!("n{raft_id}")))
    }

    fn metrics(&self) -> openraft::RaftMetrics<u64, openraft::BasicNode> {
        self.raft.metrics().borrow().clone()
    }

    /// Sum of entries newly acked by followers since the previous call,
    /// from the leader's replication metrics (`M3`). When `exclude` is
    /// given, followers whose cluster id appears in the set are skipped
    /// (partition-respecting variant).
    fn replication_delta(&self, exclude: Option<&std::collections::HashSet<String>>) -> usize {
        let metrics = self.metrics();
        let mut watermark = self.replicated_watermark.lock().unwrap();
        let mut total = 0usize;
        let Some(replication) = metrics.replication.as_ref() else {
            return 0;
        };
        for (follower, acked) in replication {
            if *follower == self.config.node_id {
                continue;
            }
            if exclude.is_some_and(|excluded| {
                excluded.contains(self.cluster_id_for(*follower).as_str())
            }) {
                continue;
            }
            let acked_index = acked.map(|log_id| log_id.index).unwrap_or(0);
            let prev = watermark.entry(*follower).or_insert(0);
            if acked_index > *prev {
                total += (acked_index - *prev) as usize;
                *prev = acked_index;
            }
        }
        total
    }

    /// Single-node bootstrap: initialize raft membership with just this
    /// node, wait for self-election, and mirror the control-plane registry.
    pub fn bootstrap(&self, nodes: Vec<(String, String)>) -> Result<ClusterNodeId, OntolithError> {
        if nodes.is_empty() {
            return Err(OntolithError::InvalidArgument("bootstrap requires nodes"));
        }
        let members: BTreeMap<u64, openraft::BasicNode> = nodes
            .iter()
            .enumerate()
            .map(|(i, (id, addr))| {
                self.raft_id_for(&ClusterNodeId::new(id.clone()));
                (i as u64, openraft::BasicNode::new(addr.clone()))
            })
            .collect();
        self.initialize_members(members)?;
        self.inner.bootstrap(nodes)?;
        self.leader_id()
            .ok_or(OntolithError::InvalidState("raft bootstrap did not elect a leader"))
    }

    /// Initialize the raft cluster with the given membership (used by the
    /// in-process multi-node harness) and wait for this node to learn the
    /// leader.
    pub fn initialize_members(
        &self,
        members: BTreeMap<u64, openraft::BasicNode>,
    ) -> Result<(), OntolithError> {
        for id in members.keys() {
            self.cluster_ids
                .write()
                .unwrap()
                .entry(*id)
                .or_insert_with(|| format!("n{id}"));
        }
        let init = self.rt.block_on(self.raft.initialize(members));
        if let Err(e) = init {
            // Multi-node bootstrap: every node may call `initialize` with the
            // same membership; openraft documents `NotAllowed` as safe to
            // ignore (the cluster is already formed and in motion).
            if !matches!(
                &e,
                openraft::error::RaftError::APIError(
                    openraft::error::InitializeError::NotAllowed(_)
                )
            ) {
                return Err(OntolithError::Failed(format!("raft initialize: {e}")));
            }
        }
        self.bootstrapped.store(true, Ordering::SeqCst);
        let wait = self.raft.wait(Some(std::time::Duration::from_secs(10)));
        self.rt
            .block_on(wait.metrics(
                |m| m.current_leader.is_some() || m.state == openraft::ServerState::Leader,
                "raft cluster leader elected",
            ))
            .map_err(|e| OntolithError::Failed(format!("raft leader wait: {e}")))?;
        Ok(())
    }

    /// Return `Ok(())` when this node is the raft leader.
    fn require_leader(&self) -> Result<(), OntolithError> {
        if self.metrics().current_leader == Some(self.config.node_id) {
            Ok(())
        } else {
            Err(OntolithError::Failed(format!(
                "not the raft leader (leader={:?})",
                self.metrics().current_leader
            )))
        }
    }
}

impl MetadataService for RaftClusterRuntime {
    fn membership(&self) -> Membership {
        let m = self.metrics();
        Membership {
            epoch: ClusterEpoch::new(m.current_term),
            leader_id: m.current_leader.map(|id| self.cluster_id_for(id)),
            nodes: self.inner.membership().nodes,
        }
    }

    fn shard_map(&self) -> ShardMap {
        self.inner.shard_map()
    }

    fn current_epoch(&self) -> ClusterEpoch {
        ClusterEpoch::new(self.metrics().current_term)
    }

    fn leader_id(&self) -> Option<ClusterNodeId> {
        self.metrics().current_leader.map(|id| self.cluster_id_for(id))
    }

    fn status(&self) -> ClusterStatus {
        let m = self.metrics();
        let nodes = self.inner.membership().nodes;
        ClusterStatus {
            epoch: ClusterEpoch::new(m.current_term),
            leader_id: m.current_leader.map(|id| self.cluster_id_for(id)),
            node_count: nodes.len(),
            healthy_count: nodes.iter().filter(|n| n.status.is_votable()).count(),
            shard_count: self.inner.shard_map().assignments.len(),
            leader_log_index: m.last_log_index.unwrap_or(0),
            commit_index: m.last_applied.map(|l| l.index).unwrap_or(0),
            failover_count: self.inner.failover_history().len(),
            partition_active: !self.inner.current_partition().is_empty(),
        }
    }

    fn register_node(&self, node: ClusterNode) -> Result<(), OntolithError> {
        let _raft_id = self.raft_id_for(&node.node_id);
        self.inner.register_node(node)
    }

    fn heartbeat(&self, node_id: &ClusterNodeId, tick: u64) -> Result<(), OntolithError> {
        self.inner.heartbeat(node_id, tick)
    }

    fn set_node_status(
        &self,
        node_id: &ClusterNodeId,
        status: crate::domain::NodeStatus,
    ) -> Result<(), OntolithError> {
        self.inner.set_node_status(node_id, status)
    }
}

impl ElectionService for RaftClusterRuntime {
    fn campaign(&self, _candidate: &ClusterNodeId) -> Result<Option<ClusterNodeId>, OntolithError> {
        if self.bootstrapped.load(Ordering::SeqCst) {
            Ok(self.leader_id())
        } else {
            Ok(None)
        }
    }

    fn step_down(&self, _leader: &ClusterNodeId) -> Result<(), OntolithError> {
        // M1: openraft owns leadership transitions; nothing to force here.
        Ok(())
    }

    fn is_leader(&self, node_id: &ClusterNodeId) -> bool {
        let raft_id = self.raft_id_for(node_id);
        self.metrics().current_leader == Some(raft_id)
    }
}

impl ShardRouter for RaftClusterRuntime {
    fn route_write(&self, key: &str) -> Result<crate::domain::WriteRoute, OntolithError> {
        self.inner.route_write(key)
    }

    fn route_read(
        &self,
        key: &str,
        consistency: ConsistencyLevel,
    ) -> Result<crate::domain::ReadRoute, OntolithError> {
        self.inner.route_read(key, consistency)
    }

    fn route_read_session(
        &self,
        key: &str,
        session: &SessionId,
        consistency: ConsistencyLevel,
    ) -> Result<crate::domain::ReadRoute, OntolithError> {
        self.inner.route_read_session(key, session, consistency)
    }

    fn replica_set(&self, shard_id: ShardId) -> Result<crate::domain::ReplicaSet, OntolithError> {
        self.inner.replica_set(shard_id)
    }
}

impl Replicator for RaftClusterRuntime {
    fn append(&self, payload: LogPayload) -> Result<LogEntry, OntolithError> {
        self.require_leader()?;
        let resp = self
            .rt
            .block_on(
                self.raft
                    .client_write::<tokio::sync::oneshot::error::RecvError>(payload.clone()),
            )
            .map_err(|e| OntolithError::Failed(format!("raft client_write: {e}")))?;
        Ok(LogEntry {
            index: resp.log_id.index,
            term: ClusterEpoch::new(resp.log_id.leader_id.term),
            payload,
        })
    }

    fn leader_index(&self) -> u64 {
        self.metrics().last_log_index.unwrap_or(0)
    }

    fn commit_index(&self) -> u64 {
        self.metrics().last_applied.map(|l| l.index).unwrap_or(0)
    }

    fn applied_index(&self, node_id: &ClusterNodeId) -> u64 {
        let raft_id = self.raft_id_for(node_id);
        if raft_id == self.config.node_id {
            self.commit_index()
        } else {
            // Leader perspective: the last log index this node has acked
            // (replicated) back to the leader.
            self.metrics()
                .replication
                .as_ref()
                .and_then(|r| r.get(&raft_id).copied().flatten())
                .map(|log_id| log_id.index)
                .unwrap_or(0)
        }
    }

    fn replicate_to_followers(&self) -> Result<usize, OntolithError> {
        Ok(self.replication_delta(None))
    }

    fn replicate_to_followers_respecting_partition(&self) -> Result<usize, OntolithError> {
        let isolated = self
            .inner
            .current_partition()
            .isolated
            .iter()
            .map(|n| n.as_str().to_owned())
            .collect::<std::collections::HashSet<_>>();
        Ok(self.replication_delta(Some(&isolated)))
    }

    fn entries_from(&self, index: u64) -> Vec<LogEntry> {
        self.store.applied_entries_from(index)
    }
}

impl FailoverController for RaftClusterRuntime {
    fn check_and_failover(&self, now_tick: u64) -> Result<Vec<FailoverEvent>, OntolithError> {
        self.inner.check_and_failover(now_tick)
    }

    fn failover_history(&self) -> Vec<FailoverEvent> {
        self.inner.failover_history()
    }
}

impl RebalanceService for RaftClusterRuntime {
    fn rebalance(&self) -> Result<Vec<RebalancePlan>, OntolithError> {
        self.inner.rebalance()
    }

    fn rebalance_history(&self) -> Vec<RebalancePlan> {
        self.inner.rebalance_history()
    }
}

impl DataPlaneSync for RaftClusterRuntime {
    fn transfer_snapshot(
        &self,
        source: &ClusterNodeId,
        target: &ClusterNodeId,
        shard_id: ShardId,
        slots: SlotRange,
        snapshot: SnapshotRef,
    ) -> Result<(), OntolithError> {
        self.inner
            .transfer_snapshot(source, target, shard_id, slots, snapshot)
    }

    fn pending_syncs(&self) -> usize {
        self.inner.pending_syncs()
    }

    fn drain_syncs(&self) -> Result<Vec<SyncReceipt>, OntolithError> {
        self.inner.drain_syncs()
    }

    fn sync_history(&self) -> Vec<SyncReceipt> {
        self.inner.sync_history()
    }
}

impl FaultInjector for RaftClusterRuntime {
    fn inject_partition(&self, isolated: Vec<ClusterNodeId>) -> Result<(), OntolithError> {
        self.inner.inject_partition(isolated)
    }

    fn heal_partition(&self) -> Result<(), OntolithError> {
        self.inner.heal_partition()
    }

    fn current_partition(&self) -> NetworkPartition {
        self.inner.current_partition()
    }
}

impl ClusterRuntime for RaftClusterRuntime {}

pub fn status() -> &'static str {
    "raft"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};

    fn wait_until(cond: impl Fn() -> bool, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        cond()
    }

    fn http_node_config(
        node_id: u64,
        secret: &str,
        storage_dir: Option<&std::path::Path>,
    ) -> RaftClusterConfig {
        RaftClusterConfig {
            node_id,
            heartbeat_interval_ms: 50,
            http_listen_addr: Some("127.0.0.1:0".to_string()),
            raft_secret: secret.to_string(),
            raft_storage_path: storage_dir.map(|dir| dir.join("raft-db")),
            ..RaftClusterConfig::default()
        }
    }

    fn raw_http_request(
        addr: SocketAddr,
        secret: Option<&str>,
        path: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).expect("connect raft http");
        let auth = match secret {
            Some(s) => format!("Authorization: Bearer {s}\r\n"),
            None => String::new(),
        };
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("write head");
        stream.write_all(body).expect("write body");
        stream.flush().expect("flush");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("read response");
        let head_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(buf.len());
        let status = String::from_utf8_lossy(&buf[..head_end])
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        (status, buf[head_end..].to_vec())
    }

    #[test]
    fn http_server_enforces_shared_secret() {
        let rt = RaftClusterRuntime::new(http_node_config(0, "s3cret", None));
        let addr = rt.http_addr().expect("http addr");

        let (status, _) = raw_http_request(addr, None, "/internal/raft/vote", b"{}");
        assert_eq!(status, 401, "missing secret must be rejected");
        let (status, _) = raw_http_request(addr, Some("wrong"), "/internal/raft/vote", b"{}");
        assert_eq!(status, 401, "wrong secret must be rejected");
        let (status, _) = raw_http_request(addr, Some("s3cret"), "/internal/raft/nope", b"{}");
        assert_eq!(status, 404, "unknown endpoint under valid secret");
    }

    #[test]
    fn http_install_snapshot_rpc_roundtrips() {
        let rt = RaftClusterRuntime::new(http_node_config(0, "snap-secret", None));
        let addr = rt.http_addr().expect("http addr");
        rt.bootstrap(vec![("n0".into(), format!("http://{addr}"))])
            .expect("bootstrap");

        // A stale-vote snapshot chunk is accepted (vote ignored, current vote
        // echoed back), proving the full serde round-trip over HTTP.
        let req = openraft::raft::InstallSnapshotRequest::<TypeConfig> {
            vote: openraft::Vote::new(0, 0),
            meta: openraft::SnapshotMeta {
                last_log_id: None,
                last_membership: openraft::StoredMembership::default(),
                snapshot_id: "test-snap".to_string(),
            },
            offset: 0,
            data: Vec::new(),
            done: false,
        };
        let body = serde_json::to_vec(&req).expect("serialize install snapshot request");
        let (status, resp_body) = raw_http_request(
            addr,
            Some("snap-secret"),
            "/internal/raft/install-snapshot",
            &body,
        );
        assert_eq!(status, 200, "stale-vote snapshot chunk must be answered");
        let resp: openraft::raft::InstallSnapshotResponse<NodeId> =
            serde_json::from_slice(&resp_body).expect("deserialize response");
        assert!(resp.vote.leader_id.term >= 1, "current term echoed back");
    }

    #[test]
    fn http_two_node_cluster_replicates_with_rocksdb() {
        let dir0 = tempfile::tempdir().unwrap();
        let dir1 = tempfile::tempdir().unwrap();
        let rt0 = RaftClusterRuntime::new(http_node_config(0, "cluster-secret", Some(dir0.path())));
        let rt1 = RaftClusterRuntime::new(http_node_config(1, "cluster-secret", Some(dir1.path())));
        let addr0 = rt0.http_addr().expect("rt0 http addr");
        let addr1 = rt1.http_addr().expect("rt1 http addr");

        rt0.bootstrap(vec![
            ("n0".into(), format!("http://{addr0}")),
            ("n1".into(), format!("http://{addr1}")),
        ])
        .expect("bootstrap over http");

        assert!(
            wait_until(
                || rt0.leader_id().is_some() || rt1.leader_id().is_some(),
                20000
            ),
            "no leader elected over HTTP"
        );
        let leader = if rt0.leader_id().is_some() { &rt0 } else { &rt1 };
        let follower = if rt0.leader_id().is_some() { &rt1 } else { &rt0 };

        // index 2: index 1 is the membership entry written by initialize().
        let entry = leader
            .append(LogPayload::Metadata("over-http".into()))
            .expect("append over http");
        assert_eq!(entry.index, 2);
        assert!(
            wait_until(|| follower.commit_index() >= 2, 20000),
            "follower did not commit replicated entry"
        );
        assert_eq!(follower.commit_index(), leader.commit_index());

        // The follower materialized the entry in its RocksDB `raft` CF.
        let replicated = follower.entries_from(2);
        assert_eq!(replicated.len(), 1);
        assert_eq!(replicated[0].payload, LogPayload::Metadata("over-http".into()));
    }

    #[test]
    fn http_three_node_cluster_majority_commit_survives_follower_loss() {
        let dirs = [
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        ];
        let mut rts = vec![
            RaftClusterRuntime::new(http_node_config(0, "cluster-secret", Some(dirs[0].path()))),
            RaftClusterRuntime::new(http_node_config(1, "cluster-secret", Some(dirs[1].path()))),
            RaftClusterRuntime::new(http_node_config(2, "cluster-secret", Some(dirs[2].path()))),
        ];
        let addrs = rts
            .iter()
            .map(|rt| rt.http_addr().expect("http addr"))
            .collect::<Vec<_>>();
        let members = (0..3)
            .map(|j| (format!("n{j}"), format!("http://{}", addrs[j])))
            .collect::<Vec<_>>();

        // Every node bootstraps the same 3-node membership; `NotAllowed`
        // (already initialized) is tolerated per openraft docs.
        for rt in &rts {
            rt.bootstrap(members.clone())
                .expect("bootstrap 3-node membership over http");
        }

        assert!(
            wait_until(
                || rts.iter().any(|rt| rt.leader_id().is_some()),
                30000
            ),
            "no leader elected in 3-node cluster"
        );
        let leader = rts
            .iter()
            .find(|rt| rt.leader_id().is_some())
            .expect("leader");
        let leader_index = rts.iter().position(|rt| std::ptr::eq(rt, leader)).unwrap();
        let follower_ids = (0..3)
            .filter(|i| *i != leader_index)
            .map(|i| rts[i].node_id())
            .collect::<Vec<_>>();

        // Index 2: index 1 is the membership entry written by initialize().
        let entry = leader
            .append(LogPayload::Metadata("majority-commit".into()))
            .expect("append over http");
        assert_eq!(entry.index, 2);
        assert!(
            wait_until(
                || rts.iter().all(|rt| rt.commit_index() >= 2),
                30000
            ),
            "all 3 nodes did not commit replicated entry"
        );
        assert!(
            leader.replicate_to_followers().unwrap() >= 1,
            "replicate_to_followers must report real replicated entries"
        );
        for follower_id in &follower_ids {
            assert!(
                leader.applied_index(&ClusterNodeId::new(format!("n{follower_id}"))) >= 2,
                "leader must observe follower acked index"
            );
        }

        // Lose one follower: majority (2 of 3) still commits.
        drop(rts.remove(2));
        assert!(
            wait_until(
                || rts.iter().any(|rt| rt.leader_id().is_some()),
                30000
            ),
            "no leader after follower loss"
        );
        let leader = rts
            .iter()
            .find(|rt| rt.leader_id().is_some())
            .expect("leader after follower loss");
        let entry = leader
            .append(LogPayload::Metadata("survives-loss".into()))
            .expect("append with 2 of 3 alive");
        assert_eq!(entry.index, 3);
        assert!(
            wait_until(
                || rts.iter().all(|rt| rt.commit_index() >= 3),
                30000
            ),
            "majority commit failed after one follower lost"
        );
    }

    #[test]
    fn single_node_bootstrap_elects_leader() {
        let rt = RaftClusterRuntime::with_defaults();
        let leader = rt.bootstrap(vec![("n0".into(), "mem://n0".into())]).unwrap();
        assert_eq!(leader.as_str(), "n0");
        assert!(rt.is_leader(&ClusterNodeId::new("n0")));
        assert_eq!(rt.leader_id(), Some(ClusterNodeId::new("n0")));
        assert!(rt.current_epoch().get() >= 1);
    }

    #[test]
    fn append_commits_through_raft() {
        let rt = RaftClusterRuntime::with_defaults();
        rt.bootstrap(vec![("n0".into(), "mem://n0".into())]).unwrap();

        // index 1 is the membership entry written by initialize().
        let e1 = rt.append(LogPayload::Metadata("alpha".into())).unwrap();
        assert_eq!(e1.index, 2);
        assert_eq!(e1.payload, LogPayload::Metadata("alpha".into()));

        let e2 = rt
            .append(LogPayload::Data {
                shard_id: ShardId::new(0),
                op: "write".into(),
            })
            .unwrap();
        assert_eq!(e2.index, 3);

        assert!(wait_until(|| rt.commit_index() == 3, 5000));
        assert_eq!(rt.leader_index(), 3);
        assert_eq!(rt.commit_index(), 3);

        let entries = rt.entries_from(1);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].payload, LogPayload::Metadata("alpha".into()));
    }

    #[test]
    fn trait_adapter_roundtrip() {
        let rt = RaftClusterRuntime::with_defaults();
        rt.bootstrap(vec![("n0".into(), "mem://n0".into())]).unwrap();

        let status = rt.status();
        assert_eq!(status.node_count, 1);
        assert_eq!(status.leader_id, Some(ClusterNodeId::new("n0")));

        let write = rt.route_write("tenant:acme").unwrap();
        assert_eq!(write.leader_node.as_str(), "n0");

        let read = rt
            .route_read("tenant:acme", ConsistencyLevel::Strong)
            .unwrap();
        assert!(read.served_by_leader);

        let membership = rt.membership();
        assert_eq!(membership.leader_id, Some(ClusterNodeId::new("n0")));
    }

    #[test]
    fn in_memory_transport_two_node_cluster() {
        let registry = Arc::new(RaftRegistry::default());
        let rt0 = RaftClusterRuntime::new_with_registry(
            RaftClusterConfig {
                node_id: 0,
                heartbeat_interval_ms: 50,
                ..RaftClusterConfig::default()
            },
            registry.clone(),
        );
        let rt1 = RaftClusterRuntime::new_with_registry(
            RaftClusterConfig {
                node_id: 1,
                heartbeat_interval_ms: 50,
                ..RaftClusterConfig::default()
            },
            registry,
        );

        // Only one node initializes the membership; the other joins via
        // replication over the in-memory transport.
        rt0.initialize_members(BTreeMap::from([
            (0, openraft::BasicNode::new("mem://n0")),
            (1, openraft::BasicNode::new("mem://n1")),
        ]))
        .unwrap();

        // A leader must emerge among the two nodes over the in-memory transport.
        assert!(wait_until(
            || rt0.leader_id().is_some() || rt1.leader_id().is_some(),
            15000
        ));

        let leader = if rt0.leader_id().is_some() { &rt0 } else { &rt1 };
        let follower = if rt0.leader_id().is_some() { &rt1 } else { &rt0 };
        // index 2: index 1 is the membership entry written by initialize().
        let entry = leader
            .append(LogPayload::Metadata("replicated".into()))
            .unwrap();
        assert_eq!(entry.index, 2);
        assert!(wait_until(|| follower.commit_index() >= 2, 15000));
        assert_eq!(follower.commit_index(), leader.commit_index());
    }
}
