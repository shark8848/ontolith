//! Ontolith security baseline (L5).
//!
//! Auth context, deny-by-default permissions, header authenticator, and
//! audit logs (in-memory + optional file JSONL). Transport binding lives in
//! `ontolith-server`.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use infrastructure::FileAuditLog;

pub const CRATE_ID: &str = "ontolith-security";
pub const LAYER: &str = "L5-access-security";

pub fn healthcheck() -> bool {
    true
}
