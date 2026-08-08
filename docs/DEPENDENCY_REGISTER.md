# Ontolith Dependency Register

Document ID: DEP-0001  
Version: 0.1.0  
Status: Active  
Date: 2026-07-17

Tier definitions: PLAN-0001 §8 / SAS-0001 §12.

| Crate | Tier | Version policy | Owner | Purpose | Risk | Rollback / replacement |
|-------|------|----------------|-------|---------|------|------------------------|
| `rocksdb` | A | Pin exact in Cargo.lock; no `*` | storage | Durable LSM/WAL embedded store for L2 | Native build; FFI surface; disk corruption if misused | Feature-off → `InMemoryStorageEngine`; later alternate CF store |
| openraft | A | Pin exact in Cargo.lock; features `raft-backend` + `serde` | cluster | Multi-node Raft for L4 data plane (see [ADR-0004](../adr/0004-multi-process-raft-data-plane.md)); `serde` enables JSON RPC/state serialization over the M2 in-tree HTTP transport | Async runtime surface; snapshot/GC ops | Keep `InMemoryClusterRuntime` simulator + traits; feature-off fallback |
| `serde` / `serde_json` | B | Follow openraft's pin (serde 1.x in tree) | cluster; security | cluster: serialize raft RPC messages, log entries, hard state, and snapshots (M2); security: JWT claim payload encoding/parsing (P5-02) | Ecosystem-stable | Revert to openraft-only serde if a lighter codec is adopted |
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
| `ontolith-cluster` | `raft-backend` | **enabled** | Compiles openraft raft data plane + in-tree HTTP RPC + RocksDB `raft` CF storage (M1+M2 landed 2026-08-08); also enables `ontolith-storage/rocksdb-backend`; disable → in-memory simulator only |
