//! Ontolith cluster runtime (L4).
//!
//! Single-region control plane: membership, simplified leader election,
//! hash-slot shard map, log replication with lag, and automatic failover.
//! Implements strongly consistent metadata APIs and client read routing by
//! [`ontolith_core::domain::ConsistencyLevel`].

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-cluster";
pub const LAYER: &str = "L4-cluster-consistency";

pub fn healthcheck() -> bool {
    true
}
