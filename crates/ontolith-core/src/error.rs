//! Shared error type for Ontolith crates.
//!
//! Static variants stay cheap (`&'static str`). Dynamic diagnostics use
//! [`OntolithError::Failed`] so parsers and query engines can include line
//! numbers and detailed messages without a separate error stack crate.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OntolithError {
    /// Caller supplied a value that fails domain validation.
    InvalidArgument(&'static str),
    /// Object is not in a state that allows the requested operation.
    InvalidState(&'static str),
    /// Requested entity does not exist.
    NotFound(&'static str),
    /// Entity already exists where uniqueness is required.
    AlreadyExists(&'static str),
    /// Feature or operation is not implemented / not enabled.
    Unsupported(&'static str),
    /// Storage / IO style failure surfaced through abstract boundaries.
    Storage(&'static str),
    /// Dynamic diagnostic message (parse/query/plan failures with context).
    Failed(String),
}

impl OntolithError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }

    pub fn parse_at(line: usize, column: usize, message: impl AsRef<str>) -> Self {
        Self::Failed(format!(
            "parse error at {}:{}: {}",
            line,
            column,
            message.as_ref()
        ))
    }

    pub fn query(message: impl Into<String>) -> Self {
        Self::Failed(format!("query error: {}", message.into()))
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::InvalidState(_) => "invalid_state",
            Self::NotFound(_) => "not_found",
            Self::AlreadyExists(_) => "already_exists",
            Self::Unsupported(_) => "unsupported",
            Self::Storage(_) => "storage",
            Self::Failed(_) => "failed",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::InvalidArgument(msg)
            | Self::InvalidState(msg)
            | Self::NotFound(msg)
            | Self::AlreadyExists(msg)
            | Self::Unsupported(msg)
            | Self::Storage(msg) => msg,
            Self::Failed(msg) => msg.as_str(),
        }
    }
}

impl std::fmt::Display for OntolithError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for OntolithError {}
