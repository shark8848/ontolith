//! Client-visible consistency levels (SAS-0001 §8).
//!
//! These are API-layer declarations. Storage and cluster adapters interpret
//! them when serving reads; single-node memory engines treat Strong and
//! Session equivalently for committed data.

/// Read consistency requested by a client or internal service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConsistencyLevel {
    /// Linearizable on the leader for committed data.
    #[default]
    Strong,
    /// Monotonic reads within a client session.
    Session,
    /// May read from followers / local replicas; staleness bounds exposed
    /// by the serving layer when available.
    Eventual,
}

impl ConsistencyLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Session => "session",
            Self::Eventual => "eventual",
        }
    }

    /// Whether this level requires a leader / primary for single-shard reads.
    pub const fn requires_primary(self) -> bool {
        matches!(self, Self::Strong)
    }
}

impl std::fmt::Display for ConsistencyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
