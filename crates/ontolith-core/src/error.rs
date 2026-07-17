#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OntolithError {
    InvalidState(&'static str),
    Unsupported(&'static str),
}
