use ontolith_storage::domain::SnapshotRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShardId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    Leader,
    Follower,
    Learner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterNode {
    pub node_id: String,
    pub address: String,
    pub role: NodeRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaSet {
    pub shard_id: ShardId,
    pub leader_id: String,
    pub follower_ids: Vec<String>,
    pub last_snapshot: Option<SnapshotRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterEpoch(pub u64);

pub fn status() -> &'static str {
    "domain"
}
