pub mod domain;
pub mod application;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-rdf";

pub fn healthcheck() -> bool {
    true
}
