//! Cluster domain model (L4 — single-region MVP).

use ontolith_core::domain::ConsistencyLevel;
use ontolith_storage::domain::SnapshotRef;

/// Logical region identifier (single-region MVP uses one region).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegionId(pub String);

impl RegionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn default_region() -> Self {
        Self::new("default")
    }
}

/// Cluster node identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClusterNodeId(pub String);

impl ClusterNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShardId(pub u32);

impl ShardId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Raft-like term / epoch for metadata leadership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ClusterEpoch(pub u64);

impl ClusterEpoch {
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeRole {
    Leader,
    Follower,
    Learner,
    Candidate,
}

impl NodeRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Leader => "leader",
            Self::Follower => "follower",
            Self::Learner => "learner",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeStatus {
    Healthy,
    Suspect,
    Dead,
}

impl NodeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Suspect => "suspect",
            Self::Dead => "dead",
        }
    }

    pub const fn is_votable(self) -> bool {
        matches!(self, Self::Healthy | Self::Suspect)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterNode {
    pub node_id: ClusterNodeId,
    pub address: String,
    pub region: RegionId,
    pub role: NodeRole,
    pub status: NodeStatus,
    /// Last heartbeat logical clock (ms or tick).
    pub last_heartbeat: u64,
}

impl ClusterNode {
    pub fn new(node_id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            node_id: ClusterNodeId::new(node_id),
            address: address.into(),
            region: RegionId::default_region(),
            role: NodeRole::Follower,
            status: NodeStatus::Healthy,
            last_heartbeat: 0,
        }
    }
}

/// Replica set for one shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSet {
    pub shard_id: ShardId,
    pub leader_id: Option<ClusterNodeId>,
    pub follower_ids: Vec<ClusterNodeId>,
    pub learner_ids: Vec<ClusterNodeId>,
    pub last_snapshot: Option<SnapshotRef>,
    /// Follower -> applied log index lag relative to leader.
    pub replica_lag: Vec<(ClusterNodeId, u64)>,
}

impl ReplicaSet {
    pub fn members(&self) -> Vec<ClusterNodeId> {
        let mut m = Vec::new();
        if let Some(l) = &self.leader_id {
            m.push(l.clone());
        }
        m.extend(self.follower_ids.iter().cloned());
        m
    }

    pub fn lag_of(&self, node: &ClusterNodeId) -> Option<u64> {
        self.replica_lag
            .iter()
            .find(|(id, _)| id == node)
            .map(|(_, lag)| *lag)
    }
}

/// Hash-slot range assigned to a shard (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRange {
    pub start: u32,
    pub end: u32,
}

impl SlotRange {
    pub fn contains(self, slot: u32) -> bool {
        slot >= self.start && slot <= self.end
    }
}

/// Shard routing entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardAssignment {
    pub shard_id: ShardId,
    pub slots: SlotRange,
    pub replica_set: ReplicaSet,
}

/// Cluster-wide shard map (metadata).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShardMap {
    pub epoch: ClusterEpoch,
    pub slot_count: u32,
    pub assignments: Vec<ShardAssignment>,
}

impl ShardMap {
    pub fn shard_for_slot(&self, slot: u32) -> Option<&ShardAssignment> {
        self.assignments
            .iter()
            .find(|a| a.slots.contains(slot % self.slot_count.max(1)))
    }

    pub fn shard_for_key(&self, key: &str) -> Option<&ShardAssignment> {
        let slot = hash_slot(key, self.slot_count.max(1));
        self.shard_for_slot(slot)
    }
}

/// Stable hash slot in `[0, slot_count)`.
pub fn hash_slot(key: &str, slot_count: u32) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in key.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(16777619);
    }
    hash % slot_count.max(1)
}

/// Replicated log entry (metadata or data op placeholder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub index: u64,
    pub term: ClusterEpoch,
    pub payload: LogPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogPayload {
    /// No-op heartbeat / barrier.
    Noop,
    /// Metadata mutation description (opaque for MVP).
    Metadata(String),
    /// Data write op reference (shard-local).
    Data { shard_id: ShardId, op: String },
}

/// Result of routing a read under a consistency level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRoute {
    pub shard_id: ShardId,
    pub target_node: ClusterNodeId,
    pub consistency: ConsistencyLevel,
    pub served_by_leader: bool,
    /// Optional max acceptable lag for eventual/session reads.
    pub max_staleness_index: Option<u64>,
}

/// Result of routing a write (always leader).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRoute {
    pub shard_id: ShardId,
    pub leader_node: ClusterNodeId,
}

/// Cluster membership snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Membership {
    pub epoch: ClusterEpoch,
    pub leader_id: Option<ClusterNodeId>,
    pub nodes: Vec<ClusterNode>,
}

impl Membership {
    pub fn healthy_voters(&self) -> Vec<&ClusterNode> {
        self.nodes
            .iter()
            .filter(|n| n.status.is_votable() && !matches!(n.role, NodeRole::Learner))
            .collect()
    }

    pub fn get(&self, id: &ClusterNodeId) -> Option<&ClusterNode> {
        self.nodes.iter().find(|n| &n.node_id == id)
    }
}

/// Failover event for observability / tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailoverEvent {
    pub at_tick: u64,
    pub shard_id: ShardId,
    pub old_leader: Option<ClusterNodeId>,
    pub new_leader: ClusterNodeId,
    pub reason: String,
}

/// Client session for sticky Session-consistency reads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Rebalance plan: move a contiguous slot range to another shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebalancePlan {
    pub from_shard: ShardId,
    pub to_shard: ShardId,
    pub slots: SlotRange,
    pub reason: String,
}

/// Network partition set: nodes that cannot vote/replicate with the majority.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkPartition {
    /// Node ids isolated from the primary partition (minority side).
    pub isolated: Vec<ClusterNodeId>,
}

impl NetworkPartition {
    pub fn is_isolated(&self, id: &ClusterNodeId) -> bool {
        self.isolated.iter().any(|n| n == id)
    }

    pub fn is_empty(&self) -> bool {
        self.isolated.is_empty()
    }
}

/// Aggregate cluster status for APIs / dashboards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterStatus {
    pub epoch: ClusterEpoch,
    pub leader_id: Option<ClusterNodeId>,
    pub node_count: usize,
    pub healthy_count: usize,
    pub shard_count: usize,
    pub leader_log_index: u64,
    pub commit_index: u64,
    pub failover_count: usize,
    pub partition_active: bool,
}

pub fn status() -> &'static str {
    "domain"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_slot_stable() {
        let a = hash_slot("tenant:acme", 1024);
        let b = hash_slot("tenant:acme", 1024);
        assert_eq!(a, b);
        assert!(a < 1024);
    }

    #[test]
    fn slot_range_contains() {
        let r = SlotRange { start: 0, end: 511 };
        assert!(r.contains(0));
        assert!(r.contains(511));
        assert!(!r.contains(512));
    }
}
