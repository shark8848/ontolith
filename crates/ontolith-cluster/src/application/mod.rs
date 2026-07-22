//! Cluster application contracts (L4).

use crate::domain::{
    ClusterEpoch, ClusterNode, ClusterNodeId, ClusterStatus, FailoverEvent, LogEntry, Membership,
    NetworkPartition, ReadRoute, RebalancePlan, ReplicaSet, SessionId, ShardId, ShardMap,
    WriteRoute,
};
use ontolith_core::domain::ConsistencyLevel;
use ontolith_core::error::OntolithError;

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
    + FaultInjector
{
    fn tick(&self, now_tick: u64) -> Result<Vec<FailoverEvent>, OntolithError> {
        self.check_and_failover(now_tick)
    }
}

pub fn status() -> &'static str {
    "application"
}
