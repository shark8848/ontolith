//! Stable identity primitives for Knowledge Objects (SAS-0401 §11).
//!
//! Logical identifiers remain stable across export, replication, and migration.
//! Internal identifiers (e.g. dictionary `NodeId`) may be backend-local.

use crate::domain::TimestampMs;

/// Immutable logical identifier of a Knowledge Object.
///
/// Stored as a non-empty UTF-8 string so it can carry IRI-style or UUID-style
/// identities without forcing a single encoding scheme at this layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("object id must not be empty");
        }
        Ok(Self(value))
    }

    /// Construct without validation. Prefer [`ObjectId::new`] at API boundaries.
    pub fn from_validated(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ObjectId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Monotonic object version within a single object lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ObjectVersion(pub u64);

impl ObjectVersion {
    pub const INITIAL: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Distinct version identifier used in history / snapshot lineages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionId(pub u64);

impl VersionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Runtime category of a Knowledge Object (SAS-0401 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectType {
    Resource,
    Statement,
    Graph,
    Dataset,
    Ontology,
    Rule,
    Version,
    Metadata,
}

impl ObjectType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "resource",
            Self::Statement => "statement",
            Self::Graph => "graph",
            Self::Dataset => "dataset",
            Self::Ontology => "ontology",
            Self::Rule => "rule",
            Self::Version => "version",
            Self::Metadata => "metadata",
        }
    }
}

impl std::fmt::Display for ObjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle state machine for Knowledge Objects (SAS-0401 §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ObjectState {
    #[default]
    Created,
    Persisted,
    Indexed,
    Replicated,
    Versioned,
    Archived,
    Deleted,
}

impl ObjectState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Persisted => "persisted",
            Self::Indexed => "indexed",
            Self::Replicated => "replicated",
            Self::Versioned => "versioned",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }

    /// Whether a transition is allowed under the normative lifecycle order.
    ///
    /// Logical deletion may occur from any non-deleted state.
    /// Archival is allowed from persisted/indexed/replicated/versioned states.
    /// Self-transitions are allowed (idempotent no-op at the policy level).
    pub const fn can_transition_to(self, next: Self) -> bool {
        use ObjectState::*;
        match (self, next) {
            (a, b)
                if matches!(
                    (a, b),
                    (Created, Created)
                        | (Persisted, Persisted)
                        | (Indexed, Indexed)
                        | (Replicated, Replicated)
                        | (Versioned, Versioned)
                        | (Archived, Archived)
                        | (Deleted, Deleted)
                ) =>
            {
                true
            }
            (Deleted, _) => false,
            (_, Deleted) => true,
            (Created, Persisted) => true,
            (Persisted, Indexed) => true,
            (Indexed, Replicated) => true,
            (Replicated, Versioned) => true,
            (Persisted | Indexed | Replicated | Versioned, Archived) => true,
            (Archived, Versioned) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for ObjectState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Common header carried by every Knowledge Object (SAS-0401 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeObjectHeader {
    pub id: ObjectId,
    pub object_type: ObjectType,
    pub version: ObjectVersion,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub state: ObjectState,
}

impl KnowledgeObjectHeader {
    pub fn new(id: ObjectId, object_type: ObjectType, created_at: TimestampMs) -> Self {
        Self {
            id,
            object_type,
            version: ObjectVersion::INITIAL,
            created_at,
            updated_at: created_at,
            state: ObjectState::Created,
        }
    }

    pub fn touch(&mut self, at: TimestampMs) {
        self.updated_at = at;
        self.version = self.version.next();
    }

    pub fn transition_to(
        &mut self,
        next: ObjectState,
        at: TimestampMs,
    ) -> Result<(), &'static str> {
        if !self.state.can_transition_to(next) {
            return Err("invalid knowledge object lifecycle transition");
        }
        self.state = next;
        self.updated_at = at;
        self.version = self.version.next();
        Ok(())
    }
}
