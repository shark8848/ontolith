//! Ontolith transaction coordinator (L2 companion).
//!
//! Manages txn identity, mode, lifecycle, timeouts and active limits.
//! Storage visibility rules are documented in the L2 storage contract.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-transaction";
pub const LAYER: &str = "L2-storage-transaction-kernel";

pub fn healthcheck() -> bool {
    true
}
