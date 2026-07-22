pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-reasoner";

pub fn healthcheck() -> bool {
    true
}
