pub mod domain;
pub mod application;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-query";

pub fn healthcheck() -> bool {
    true
}
