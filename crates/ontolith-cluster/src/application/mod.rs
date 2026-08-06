//! Cluster application contracts (L4).

use crate::domain::{
    ClusterEpoch, ClusterNode, ClusterNodeId, ClusterStatus, FailoverEvent, LogEntry, Membership,
    NetworkPartition, ReadRoute, RebalancePlan, ReplicaSet, SessionId, ShardId, ShardMap,
    SyncReceipt, WriteRoute,
};
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;
use ontolith_storage::domain::SnapshotRef;

/// Strongly consistent metadata service (control plane).
pub trait MetadataService: Send + Sync {
    fn membership(&self) -> Membership;
    fn shard_map(&self) -> ShardMap;
    fn current_epoch(&self) -> ClusterEpoch;
    fn leader_id(&self) -> Option<ClusterNodeId>;
    fn status(&self) -> ClusterStatus;

    fn register_node(&self, node: ClusterNode) -> Result<(), OntolithError>;
    fn heartbeat(&self, node_id: &ClusterNodeId, tick: u64) -> Result<(), OntolithError>;
    fn set_node_status(
        &self,
        node_id: &ClusterNodeId,
        status: crate::domain::NodeStatus,
    ) -> Result<(), OntolithError>;
}

/// Leader election for the metadata / control group.
pub trait ElectionService: Send + Sync {
    /// Run one election round; returns new leader if elected.
    fn campaign(&self, candidate: &ClusterNodeId) -> Result<Option<ClusterNodeId>, OntolithError>;
    fn step_down(&self, leader: &ClusterNodeId) -> Result<(), OntolithError>;
    fn is_leader(&self, node_id: &ClusterNodeId) -> bool;
}

/// Shard placement and key routing.
pub trait ShardRouter: Send + Sync {
    fn route_write(&self, key: &str) -> Result<WriteRoute, OntolithError>;
    fn route_read(
        &self,
        key: &str,
        consistency: ConsistencyLevel,
    ) -> Result<ReadRoute, OntolithError>;
    /// Session-sticky read: prefers last node for the session when still valid.
    fn route_read_session(
        &self,
        key: &str,
        session: &SessionId,
        consistency: ConsistencyLevel,
    ) -> Result<ReadRoute, OntolithError>;
    fn replica_set(&self, shard_id: ShardId) -> Result<ReplicaSet, OntolithError>;
}

/// Append-only replication log with follower apply and quorum commit.
pub trait Replicator: Send + Sync {
    fn append(&self, payload: crate::domain::LogPayload) -> Result<LogEntry, OntolithError>;
    fn leader_index(&self) -> u64;
    /// Highest index known to be applied on a majority of voters.
    fn commit_index(&self) -> u64;
    fn applied_index(&self, node_id: &ClusterNodeId) -> u64;
    /// Push unapplied entries to followers; returns how many entries applied total.
    fn replicate_to_followers(&self) -> Result<usize, OntolithError>;
    /// Replicate only to nodes not isolated by the current partition.
    fn replicate_to_followers_respecting_partition(&self) -> Result<usize, OntolithError>;
    fn entries_from(&self, index: u64) -> Vec<LogEntry>;
}

/// Detect dead leaders and promote a follower.
pub trait FailoverController: Send + Sync {
    fn check_and_failover(&self, now_tick: u64) -> Result<Vec<FailoverEvent>, OntolithError>;
    fn failover_history(&self) -> Vec<FailoverEvent>;
}

/// Online slot rebalance (control-plane only in MVP).
pub trait RebalanceService: Send + Sync {
    /// Evenly redistribute slots across shards; returns applied plans.
    fn rebalance(&self) -> Result<Vec<RebalancePlan>, OntolithError>;
    fn rebalance_history(&self) -> Vec<RebalancePlan>;
}

/// Snapshot-based data migration between shard owners (data plane).
///
/// This is the missing half of online rebalance: control-plane slot
/// reassignment ([`RebalanceService`]) plus actual data handoff. The MVP
/// implementation simulates the transfer in-process; multi-process RPC
/// streams snapshot + log entries behind the same trait (ADR-0002 follow-up).
pub trait DataPlaneSync: Send + Sync {
    /// Queue a slot-range snapshot transfer from source to target node.
    fn transfer_snapshot(
        &self,
        source: &ClusterNodeId,
        target: &ClusterNodeId,
        shard_id: ShardId,
        slots: crate::domain::SlotRange,
        snapshot: SnapshotRef,
    ) -> Result<(), OntolithError>;

    /// Number of queued, not-yet-completed transfers.
    fn pending_syncs(&self) -> usize;

    /// Complete all pending transfers; returns receipts in completion order.
    fn drain_syncs(&self) -> Result<Vec<SyncReceipt>, OntolithError>;

    fn sync_history(&self) -> Vec<SyncReceipt>;
}

/// Fault-injection for tests and chaos demos.
pub trait FaultInjector: Send + Sync {
    fn inject_partition(&self, isolated: Vec<ClusterNodeId>) -> Result<(), OntolithError>;
    fn heal_partition(&self) -> Result<(), OntolithError>;
    fn current_partition(&self) -> NetworkPartition;
}

/// Composite single-region cluster runtime surface.
pub trait ClusterRuntime:
    MetadataService
    + ElectionService
    + ShardRouter
    + Replicator
    + FailoverController
    + RebalanceService
    + DataPlaneSync
    + FaultInjector
{
    fn tick(&self, now_tick: u64) -> Result<Vec<FailoverEvent>, OntolithError> {
        self.check_and_failover(now_tick)
    }
}

pub fn status() -> &'static str {
    "application"
}
