# ADR-0002: In-process single-region cluster MVP (no openraft yet)

- Status: Accepted
- Date: 2026-07-17
- Tags: cluster, raft, l4

## Context

Phase 4 requires metadata leadership, sharding, replication, and failover for R1
single-region demos. Introducing a full multi-node Raft stack (openraft, etc.)
now would add a Tier A dependency, async runtime coupling, and operational
surface before L5/L7 are ready.

## Decision

1. Ship **`InMemoryClusterRuntime`** implementing Metadata / Election / Router /
   Replicator / Failover traits inside `ontolith-cluster`.
2. Use simplified quorum election and heartbeat-based failure detection.
3. Defer multi-process Raft and network RPC to a later ADR when multi-node
   deployment is scheduled.
4. Keep client consistency routing API stable (`ConsistencyLevel`) so L5 can
   integrate without rewrite.

## Consequences

- Fast unit tests and local demos of failover / routing.
- Not production HA: single process, memory state.
- Migration path: replace infrastructure adapter with openraft-backed service
  behind the same traits.

## Alternatives

| Option | Why not now |
|--------|-------------|
| openraft immediately | Heavy dep + async; premature |
| No cluster code until multi-node | Blocks R1 demo of failover/routing APIs |
