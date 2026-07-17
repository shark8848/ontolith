pub mod domain;
pub mod application;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-parser";

pub fn healthcheck() -> bool {
    true
}
