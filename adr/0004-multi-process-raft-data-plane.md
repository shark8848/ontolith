# ADR-0004: Multi-process Raft data plane (openraft behind cluster traits)

- Status: Accepted
- Date: 2026-08-06
- Deciders: Codex (workspace) — pending PLAN-0001 signature review
- Tags: cluster, raft, l4, tier-a

## Context

R1 exit criteria require a **multi-node data plane**: real consensus across
processes, not only the in-process simulator. ADR-0002 explicitly deferred
multi-process Raft ("Defer multi-process Raft and network RPC to a later ADR
when multi-node deployment is scheduled").

Current state:

- `InMemoryClusterRuntime` implements `MetadataService` / `ElectionService` /
  `ShardRouter` / `Replicator` / `FailoverController` inside one process
  (fake quorum via tick counters, no network).
- `DataPlaneSync` already models snapshot migration queue + receipts.
- L2 `RocksDbStorageEngine` is a Tier A dependency with WAL + CF support
  (ADR-0001); L5 serves HTTP through axum.
- L4 document marks "多进程 openraft" as 延期 (deferred) pending this ADR.

Forces:

- R1 "单区域集群核心" wants an operable multi-node demo (≥3 processes).
- A hand-rolled Raft is a correctness liability; a maintained library
  (openraft) is the pragmatic Tier A choice, consistent with the Rust-only
  production path and the existing trait-isolation policy.
- The async runtime (tokio) is already required for the HTTP stack (axum); a
  raft RPC channel on the same stack avoids new runtime coupling.

## Decision

1. **Adopt `openraft`** as the consensus engine for the multi-process data
   plane (replaces the simulated quorum in `InMemoryClusterRuntime`), behind
   the existing cluster application traits so L5 `/cluster` API and
   `ConsistencyLevel` routing stay stable.
2. **Transport = in-tree HTTP RPC** on the existing axum/reqwest stack:
   dedicated `/internal/raft/{append,append-entries,vote,install-snapshot}`
   endpoints, peer-to-peer, authenticated by a shared cluster secret
   (`ONTOLITH_CLUSTER_SECRET`, env or file) — no new gRPC/Tonic dependency.
3. **Log + snapshot storage = RocksDB** in a dedicated `raft` column family:
   openraft log entries, hard state, and snapshot refs; data snapshots reuse
   the existing `StorageEngine::snapshot_with` path. `InMemoryStorageEngine`
   remains the test fallback.
4. **Write path**: client writes (triples / metadata) become raft entries
   (replicated + majority-committed) and apply to the local L2 engine on
   commit; `Replicator::append` / `commit_index` / `replicate_to_followers`
   map to openraft's `ClientWrite` / `leader_commit` surfaces.
5. **Rollout in milestones** behind the traits, simulator kept as the
   deterministic test harness:
   - M1: openraft single-node bootstrap + trait adapter + in-memory transport.
   - M2: multi-process HTTP RPC + RocksDB raft CF + snapshot install.
   - M3: default runtime switch to `RaftClusterRuntime`; `InMemoryClusterRuntime`
     demoted to test/CI harness; CI multi-node smoke (3 processes).
6. **Scope for R1**: single-region, fixed membership bootstrap via config
   file; dynamic membership/learner ops are follow-up.

## Consequences

### Positive

- Real majority-committed consensus across processes; R1 multi-node data
  plane becomes demonstrable and CI-gated.
- Correctness (election, log matching, snapshot, membership) from a
  maintained implementation instead of in-house re-invention.
- Stable API: L5 and tests keep talking to the same cluster traits.

### Negative / risks

- New Tier A dependency (openraft) + tokio async surface in `ontolith-cluster`.
- Operational surface: snapshot/compaction, raft dir sizing, log GC.
- Multi-process determinism is weaker than the simulator for partition
  injection tests (needs real network fault injection).

### Mitigations

- Trait isolation: only `ontolith-cluster::infrastructure::raft` touches
  openraft; simulator retained for deterministic unit tests.
- DEPENDENCY_REGISTER entry + pin exact version in `Cargo.lock`; feature
  flag `raft-backend` (default on) with in-memory fallback.
- Snapshot policy (threshold + interval) tuned in M2; log GC follows openraft
  `purge_log` guidance; CI multi-node smoke keeps snapshot path exercised.
- Keep R1 scope single-region/fixed membership to limit blast radius.

## Alternatives considered

| Option | Why not now |
|--------|-------------|
| Hand-rolled Raft | Correctness/verification cost too high for R1; risk of subtle liveness bugs |
| `raft-rs` (tikv) | No snapshot/membership ergonomics comparable to openraft; less active for app-level integration |
| gRPC (tonic) transport | New Tier A dependency + codegen; axum/reqwest already in tree and sufficient for peer RPC |
| Keep simulator only | Fails the R1 multi-node data-plane exit criterion |

## References

- [ADR-0002](./0002-cluster-mvp-in-process.md) (superseded for the data plane;
  simulator retained as harness)
- [ADR-0001](./0001-rocksdb-storage-backend.md) (RocksDB Tier A basis)
- PLAN-0001 Phase 4 / WBS-06 / R1 "单区域集群核心"
- [L4 文档](../docs/L4-ontolith-cluster-consistency.md) 边界 §6
- [DEPENDENCY_REGISTER](../docs/DEPENDENCY_REGISTER.md)
