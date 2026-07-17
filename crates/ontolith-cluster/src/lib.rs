pub mod domain;
pub mod application;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-cluster";

pub fn healthcheck() -> bool {
    true
}
