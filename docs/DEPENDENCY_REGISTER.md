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
| tonic / prost | A | Pin exact in Cargo.lock; features `grpc-backend` + `tonic-build`/`protoc-bin-vendored` build deps | server | gRPC access boundary for L5/P5-01: `SparqlService{Query,Health}` over HTTP/2 (tonic 0.12 + prost 0.13); `tonic-build` compiles `proto/ontolith/v1/sparql.proto`, `protoc-bin-vendored` vendors the protoc binary so builds need no system protoc | Async runtime surface; generated-code churn on proto change; HTTP/2 TLS via rustls not yet wired for gRPC | Feature-off → HTTP gateway only (`--no-default-features`); later axum/tonic-rs alternative |
| tokio | A | Pin exact in Cargo.lock (tonic's async runtime, features `rt-multi-thread`/`macros`/`net`/`time`/`sync`/`io-util`) | server | Dedicated multi-thread runtime for the gRPC server thread (`serve_grpc`) | Async runtime surface; task starvation under load | Keep the dedicated-thread model; swap to hyper/axum later |
| `protoc-bin-vendored` | B | Pin exact in Cargo.lock (build-dep of `grpc-backend`) | server | Vendors protoc 3 for `tonic-build` so `build.rs` needs no system protoc | Version skew with protoc/genproto | Use system protoc via `PROTOC` env if vendoring breaks |
| `serde` / `serde_json` | B | Follow openraft's pin (serde 1.x in tree) | cluster; security | cluster: serialize raft RPC messages, log entries, hard state, and snapshots (M2); security: JWT claim payload encoding/parsing (P5-02) | Ecosystem-stable | Revert to openraft-only serde if a lighter codec is adopted |
| `rustls` / `rustls-pemfile` | A | Pin rustls 0.23 with `ring` provider (ADR-0003) | server | Management-plane TLS termination (rustls in-process) + R2 non-loopback TLS gate; PEM cert/key loading | Crypto/version churn; feature surface | Feature-off → plaintext loopback only (R2 gate rejects non-loopback without TLS) |
| `tonic-build` | B | Pin exact in Cargo.lock (build-dep of `grpc-backend`) | server | Compiles `proto/ontolith/v1/sparql.proto` (P5-01); companion to tonic/prost | Generated-code churn on proto change | Feature-off → HTTP gateway only |
| `regex` | B | Pin exact in Cargo.lock | query | SPARQL `regex`/`FILTER` regex function subset + SHACL `sh:pattern` small-regex support | ReDoS surface (engine-side size guards) | Restrict to engine-side guardrails; fallback literal prefix match |
| `rcgen` | B | Pin exact in Cargo.lock | server | Self-signed cert generation for TLS smoke/tests (gen-self-signed-cert) | Cert formats | Use openssl CLI in ops scripts |
| `quick-xml` | B | Pin exact in Cargo.lock (dev-dep of `ontolith-compliance`) | compliance | RDF/XML result parsing for the W3C suite harness | Format churn | Keep vendored fixtures |
| `tempfile` | C | Pin exact in Cargo.lock (dev-dep) | all | Temporary dirs for tests/drills | Low | Use mktemp in shell drills |
| (workspace path crates) | A/B | path deps | platform | Internal modules | Low | N/A |

## Admission checklist (Tier A)

- [x] RFC/ADR: [ADR-0001](../adr/0001-rocksdb-storage-backend.md)
- [x] Trait isolation: only `ontolith-storage::infrastructure::rocksdb`
- [x] License: Apache-2.0 / BSD-style stack via `rocksdb` crate (verify on upgrade)
- [x] CI dependency-register audit job（P0-03，2026-08-09：`scripts/audit-dependency-register.sh` 校验全部直接依赖已登记）+ CI CVE audit（`cargo-audit` non-blocking 观测轨，Phase 7）
- [x] Fallback: in-memory engine always available

## Feature flags

| Crate | Feature | Default | Effect |
|-------|---------|---------|--------|
| `ontolith-storage` | `rocksdb-backend` | **enabled** | Compiles RocksDB adapter + integration tests |
| `ontolith-cluster` | `raft-backend` | **enabled** | Compiles openraft raft data plane + in-tree HTTP RPC + RocksDB `raft` CF storage (M1+M2 landed 2026-08-08); also enables `ontolith-storage/rocksdb-backend`; disable → in-memory simulator only |
| `ontolith-server` | `grpc-backend` | **enabled** | Compiles tonic/prost gRPC gateway (`SparqlService{Query,Health}`, P5-01) + `build.rs` protoc bootstrap; disable → HTTP gateway only |
