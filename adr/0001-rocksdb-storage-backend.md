# ADR-0001: RocksDB as L2 durable storage backend

- Status: Accepted
- Date: 2026-07-17
- Deciders: sharky-ai
- Tags: storage, tier-a, rocksdb

## Context

Ontolith L2 requires durable, recoverable writes under the `StorageEngine` /
`WriteAheadLog` / `DictionaryCodec` traits (SAS-0001 §6, PLAN Phase 2 P2-01).
The in-memory engine is production-shaped for correctness but not process-durable.

## Decision

1. Adopt **RocksDB** (Rust crate `rocksdb`) as the first durable backend.
2. Implement it **inside** `ontolith-storage` under
   `infrastructure::rocksdb`, gated by feature `rocksdb-backend`.
3. **Do not** expose `rocksdb::` types outside that module / feature boundary.
4. Recovery source of truth: committed `triples` / `quads` column families;
   WAL CF retains stage/commit/abort history for audit and tolerant replay tools.
5. Runtime reads use in-process secondary indexes rebuilt/updated from durable
   state (same six-permutation model as the memory engine) so L3 query paths
   stay unchanged.

## Consequences

### Positive

- Process crash recovery for dictionary + statements.
- Same traits as `InMemoryStorageEngine` → L3/query/server switch by factory.
- Column-family layout maps cleanly to index permutations later.

### Negative / risks

- Native build dependency (C++ RocksDB via the crate).
- Dual maintenance of memory indexes + durable CFs until pure CF scans land.
- Feature flag complexity in CI (need `rocksdb-backend` builders).

### Mitigations

- Feature-gate default on for this workspace; document system build deps.
- Keep memory engine as default unit-test backend without disk.
- Dependency register entry with owner, license, rollback (in-memory).

## Alternatives considered

| Option | Why not now |
|--------|-------------|
| Pure in-memory only | Fails R1 durability / recovery exit criteria |
| SQLite | Weaker LSM fit for multi-index key order; extra SQL impedance |
| Separate `ontolith-storage-rocksdb` crate | Premature split; same repo boundary sufficient with feature gate |

## References

- SAS-0001 §6 Storage Architecture
- PLAN-0001 Phase 2 / P2-01
- [L2 kernel doc](../docs/L2-ontolith-storage-transaction-kernel.md)
