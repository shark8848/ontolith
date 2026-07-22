//! Ontolith storage engine (L2).
//!
//! - In-memory engine: always available for tests and ephemeral runtimes.
//! - RocksDB engine (feature `rocksdb-backend`): durable CF layout behind the
//!   same `StorageEngine` / `DictionaryCodec` / `WriteAheadLog` traits.
//!   Vendor types never leave `infrastructure::rocks`.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub const CRATE_ID: &str = "ontolith-storage";
pub const LAYER: &str = "L2-storage-transaction-kernel";

pub fn healthcheck() -> bool {
    true
}

/// Factory: open durable RocksDB engine when the feature is enabled.
#[cfg(feature = "rocksdb-backend")]
pub fn open_durable_engine(
    path: impl AsRef<std::path::Path>,
) -> Result<infrastructure::RocksDbStorageEngine, ontolith_core::error::OntolithError> {
    infrastructure::open_rocksdb_engine(path)
}
