# Ontolith Dependency Register

Document ID: DEP-0001  
Version: 0.1.0  
Status: Active  
Date: 2026-07-17

Tier definitions: PLAN-0001 §8 / SAS-0001 §12.

| Crate | Tier | Version policy | Owner | Purpose | Risk | Rollback / replacement |
|-------|------|----------------|-------|---------|------|------------------------|
| `rocksdb` | A | Pin exact in Cargo.lock; no `*` | storage | Durable LSM/WAL embedded store for L2 | Native build; FFI surface; disk corruption if misused | Feature-off → `InMemoryStorageEngine`; later alternate CF store |
| openraft (future) | A | TBD | cluster | Multi-node Raft for L4 production | Ops + async surface | Keep `InMemoryClusterRuntime` traits; see ADR-0002 |
| (workspace path crates) | A/B | path deps | platform | Internal modules | Low | N/A |

## Admission checklist (Tier A)

- [x] RFC/ADR: [ADR-0001](../adr/0001-rocksdb-storage-backend.md)
- [x] Trait isolation: only `ontolith-storage::infrastructure::rocksdb`
- [x] License: Apache-2.0 / BSD-style stack via `rocksdb` crate (verify on upgrade)
- [ ] CI CVE audit job (Phase 7)
- [x] Fallback: in-memory engine always available

## Feature flags

| Crate | Feature | Default | Effect |
|-------|---------|---------|--------|
| `ontolith-storage` | `rocksdb-backend` | **enabled** | Compiles RocksDB adapter + integration tests |
