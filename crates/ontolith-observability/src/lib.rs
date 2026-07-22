pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-observability";

pub fn healthcheck() -> bool {
    true
}
