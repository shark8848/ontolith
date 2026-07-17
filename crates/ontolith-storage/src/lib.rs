pub mod domain;
pub mod application;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-storage";

pub fn healthcheck() -> bool {
    true
}
