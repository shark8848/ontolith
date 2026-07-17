pub mod domain;
pub mod application;
pub mod infrastructure;
pub mod error;

pub const CRATE_ID: &str = "ontolith-core";

pub fn healthcheck() -> bool {
    true
}
