//! Ontolith server / L5 access boundary.
//!
//! HTTP gateway over L2 storage + L3 query with L5 security hooks.

pub mod api;
pub mod app;
pub mod bootstrap;
#[cfg(feature = "grpc-backend")]
pub mod grpc;
pub mod http;
pub mod management;
pub mod reasoning;
pub mod runtime;
pub mod tenants;

pub const CRATE_ID: &str = "ontolith-server";
pub const LAYER: &str = "L5-access-security";

pub fn healthcheck() -> bool {
    true
}
