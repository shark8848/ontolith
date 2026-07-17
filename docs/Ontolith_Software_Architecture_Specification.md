# Ontolith Software Architecture Specification

**Document ID:** SAS-0001\
**Version:** 1.2.0-draft\
**Status:** Draft\
**Project:** Ontolith\
**Owner:** sharky-ai\
**Date:** 2026-07-12

------------------------------------------------------------------------

# 1. Executive Summary

## 1.1 Purpose

Ontolith is a cloud-native ontology runtime and distributed semantic
computing platform written in Rust.

Its mission is to become the reference open-source implementation for
ontology management, semantic reasoning, distributed RDF storage, and
standards-compliant knowledge graph infrastructure.

This revision introduces implementable constraints for release planning,
quality attributes, consistency guarantees, plugin contracts, and
operational readiness.

## 1.2 Vision

-   Standards First
-   Reasoning Native
-   Cloud Native
-   Rust Powered

## 1.3 Scope and Boundaries

In scope for this specification:

-   Runtime architecture and interfaces
-   Storage/query/reasoning subsystem responsibilities
-   Distribution, security, and governance constraints
-   Release-level non-functional targets
-   Rust-only implementation constraints and toolchain policy

Out of scope for this revision:

-   UI/console feature-level specification
-   Commercial packaging and licensing details
-   Full multi-region active-active semantics (deferred)
-   Production runtime components implemented in non-Rust languages

------------------------------------------------------------------------

# 2. Design Principles

## AP-001 Standards First

External behavior MUST follow W3C standards.

## AP-002 Modular Architecture

Every subsystem SHALL be independently replaceable.

## AP-003 Plugin First

Storage, parsers, serializers, optimizers, reasoners and security
providers SHALL be pluggable.

## AP-004 Distributed by Design

Every component SHALL support future distributed deployment.

## AP-005 Safety First

Unsafe Rust requires explicit architecture approval.

## AP-006 Measurable Architecture

Architecture decisions SHALL include measurable acceptance criteria.

## AP-007 Rust-Only Implementation

All production control plane and data plane components MUST be
implemented in Rust.

Non-Rust code is allowed only for:

-   Build/test automation scripts
-   Developer tooling and docs generation
-   Interoperability SDKs generated from stable API contracts
-   Embedded third-party native engines invoked through Rust adapters
      with architecture approval

------------------------------------------------------------------------

# 3. Supported Standards

-   RDF 1.2
-   RDF-star
-   SPARQL 1.1
-   OWL 2 RL
-   SHACL
-   SKOS
-   PROV-O
-   GeoSPARQL

Release conformance policy:

-   MVP (R1) MUST provide normative behavior for RDF 1.2 core model and
      SPARQL 1.1 Query.
-   OWL 2 RL, SHACL, and GeoSPARQL MAY be delivered as profile-gated
      modules before full conformance.
-   Any unsupported standard feature MUST produce deterministic,
      documented errors.

------------------------------------------------------------------------

# 4. High-Level Architecture

``` text
Applications/SDKs
        │
   API Gateway Layer
        │
   Semantic Runtime
   ├── Query Engine
   ├── Reasoning Engine
   ├── Validation Engine (SHACL)
   └── Transaction Coordinator
        │
   Storage Abstraction
   ├── Dictionary/Encoding
   ├── Triple-Quad Indexes
   └── WAL/Snapshot
        │
   Distributed Runtime
   ├── Consensus/Metadata
   ├── Shard Placement
   └── Replication/Failover
```

Control plane and data plane separation:

-   Control plane: metadata, topology, placement, policy
-   Data plane: ingest, query, update, reasoning execution

------------------------------------------------------------------------

# 5. Workspace Layout

``` text
ontolith/
├── crates/
├── docs/
├── rfc/
├── adr/
├── tests/
├── examples/
├── benchmarks/
└── deployments/
```

Core crates:

-   ontolith-core
-   ontolith-rdf
-   ontolith-parser
-   ontolith-query
-   ontolith-storage
-   ontolith-reasoner
-   ontolith-cluster
-   ontolith-server
-   ontolith-sdk

Recommended additional crates for clear ownership:

-   ontolith-transaction
-   ontolith-security
-   ontolith-observability
-   ontolith-plugin-api

Rust implementation policy:

-   Every production service SHALL map to one or more Rust crates under
      `crates/`.
-   Shared contracts SHALL be implemented as Rust libraries and reused
      across binaries.
-   Unsafe Rust usage SHALL be minimized, reviewed, and documented via
      ADR references.
-   FFI to non-Rust runtimes in production path SHOULD be avoided; if
      unavoidable, it MUST be isolated behind a Rust trait boundary.

------------------------------------------------------------------------

# 6. Storage Architecture

Pipeline:

``` text
RDF Node
 ↓
Dictionary Encoding
 ↓
Node Identifier
 ↓
Triple/Quad Encoding
 ↓
Indexes (SPO/SOP/PSO/POS/OSP/OPS)
 ↓
Storage Engine
```

Storage requirements:

-   Logical model SHALL support named graphs (quad model) with dataset
      default graph compatibility.
-   Dictionary SHALL provide stable ID mapping per snapshot epoch.
-   Writes SHALL be durable via WAL before ACK.
-   Index maintenance strategy SHALL be configurable (sync/async) with
      correctness guarantees documented.
-   Compaction, vacuum, and dictionary GC SHALL be online-safe.

------------------------------------------------------------------------

# 7. Query Pipeline

``` text
SPARQL
 ↓
Lexer
 ↓
Parser
 ↓
AST
 ↓
Algebra
 ↓
Logical Plan
 ↓
Optimizer
 ↓
Physical Plan
 ↓
Executor
 ↓
Streaming Result
```

Query planning requirements:

-   Query optimizer SHALL support rule-based optimization at minimum.
-   Cost-based optimization SHOULD be enabled when statistics are
      available.
-   Runtime SHALL expose query plan explanation in logical and physical
      forms.
-   Query cancellation and timeout SHALL be first-class APIs.

------------------------------------------------------------------------

# 8. Consistency and Transaction Model

Consistency levels (client-visible):

-   Strong read: linearizable on leader for committed data
-   Session read: monotonic within client session
-   Eventual read: follower/local read with staleness bounds exposed

Transaction semantics:

-   Single-shard writes MUST provide ACID semantics.
-   Cross-shard writes SHOULD use 2PC or deterministic transaction
      routing; failure mode and compensation rules MUST be documented.
-   Read-after-write consistency behavior MUST be explicit per API.

Failure handling:

-   Leader failover target: automated with bounded unavailability window
-   Retry semantics: idempotent APIs SHALL provide idempotency keys
-   Split-brain prevention SHALL be guaranteed by consensus membership

------------------------------------------------------------------------

# 9. Cluster Requirements

-   Raft consensus
-   Metadata service
-   Leader election
-   Sharding
-   Replication
-   Automatic failover
-   Online rebalancing

Operational constraints:

-   Rebalancing SHALL be throttled and observable.
-   Replica lag thresholds SHALL trigger backpressure or degraded-read
      policy.
-   Metadata APIs SHALL be strongly consistent.

------------------------------------------------------------------------

# 10. Plugin Contract and Extensibility

Plugin categories:

-   Storage backend
-   Parser/serializer
-   Optimizer/rewrite rules
-   Reasoner profile
-   Security provider

Contract requirements:

-   Every plugin SHALL declare capability metadata and version range.
-   Plugin API SHALL use semantic versioning with compatibility matrix.
-   Plugin implementation SHALL be Rust native dynamic library or Rust-
      compiled WASM module.
-   Plugin isolation SHALL support process-level or WASM sandbox mode.
-   Resource quotas (CPU, memory, timeout) SHALL be enforceable.
-   Plugin startup, health, and teardown lifecycle hooks SHALL be
      standardized.

Compatibility constraints:

-   Plugin ABI SHALL be Rust-defined and versioned in
      `ontolith-plugin-api`.
-   Direct plugin execution of non-Rust managed runtimes in hot path is
      prohibited.

------------------------------------------------------------------------

# 11. Security

-   TLS 1.3
-   OAuth2
-   OIDC
-   JWT
-   RBAC
-   Audit logging

Security architecture constraints:

-   Threat modeling SHALL be performed for each major release.
-   Secrets SHALL be externally managed and rotated.
-   Audit events SHALL be immutable and queryable.
-   Tenant isolation MUST be enforced in API, query, and storage layers.
-   Default policy SHALL be least privilege and deny-by-default.

------------------------------------------------------------------------

# 12. Third-Party Components and Dependency Baseline

Purpose:

-   Define approved external components, libraries, and governance
      requirements for production use.

Dependency tiers:

-   Tier A (runtime critical): directly impacts availability,
      correctness, or data durability.
-   Tier B (runtime optional): optional features with graceful
      degradation path.
-   Tier C (build/test/dev): not part of production execution path.

R1 approved baseline (initial):

-   Oxigraph (Tier A, Rust): RDF/SPARQL kernel reference implementation
      for single-node semantic execution and adapter reuse.
-   RocksDB via Rust bindings (Tier A, native engine): embedded LSM/WAL
      storage engine for durability and index persistence.
-   Tokio ecosystem (Tier A, Rust): async runtime and I/O foundation.
-   Serde ecosystem (Tier A, Rust): serialization and configuration data
      contracts.
-   Tracing ecosystem (Tier A, Rust): logs, metrics, and diagnostics
      instrumentation.

Adoption rules:

-   Every third-party component SHALL have: owner, purpose, risk level,
      fallback plan, and replacement strategy.
-   New Tier A dependency requires RFC and ADR approval before
      production rollout.
-   Any native (non-Rust) engine dependency MUST be wrapped by a stable
      Rust trait boundary and MUST NOT leak vendor-specific APIs upward.
-   License compatibility MUST be verified before merge (Apache-2.0/MIT
      preferred; strong copyleft requires explicit approval).

Version and supply-chain policy:

-   Wildcard dependency versions are prohibited in production crates.
-   `Cargo.lock` SHALL be committed for reproducible builds.
-   CVE scanning and dependency auditing SHALL run in CI.
-   Critical security vulnerability remediation target: <= 72 hours.

Portability and exit strategy:

-   Storage and query engines SHALL be consumed via internal interfaces
      (`ontolith-storage`, `ontolith-query`, `ontolith-plugin-api`).
-   Migration away from any Tier A dependency SHALL be technically
      feasible without API-layer breaking changes.

------------------------------------------------------------------------

# 13. Quality Attributes and SLO Targets

Release R1 baseline targets (production profile):

-   Availability: >= 99.9% monthly (single-region cluster)
-   Query latency: p95 <= 200 ms for bounded analytical benchmark set
-   Write latency: p95 <= 120 ms for single-shard update operations
-   Ingest throughput: >= 50K triples/sec per node (reference hardware)
-   Recovery objective: RTO <= 10 min, RPO <= 1 min

Quality gates:

-   Any release candidate failing two or more SLO gates MUST be blocked.
-   Performance claims SHALL reference reproducible benchmark profiles.

Rust quality gates:

-   `cargo fmt --check`, `cargo clippy -D warnings`, and full test suite
      SHALL pass on release branches.
-   Safety-critical crates SHOULD include Miri/sanitizer or equivalent
      memory-safety checks in CI.

------------------------------------------------------------------------

# 14. Deployment Views

Logical deployment view:

``` text
[Client SDKs]
       │
       ▼
[API Gateway] ── [AuthN/AuthZ]
       │
       ▼
[Semantic Runtime Pods (Rust)]
  ├─ Query/Reasoning Workers
  ├─ Transaction Coordinator
  └─ Plugin Host
       │
       ▼
[Storage Nodes (Rust)] <──> [Metadata/Consensus Cluster (Rust)]
       │
       ▼
[Backup/Snapshot + Observability Stack]
```

Write request sequence (simplified):

``` text
Client -> Gateway -> Runtime -> Tx Coordinator -> Storage Leader
  -> WAL fsync -> Index Update -> Replication Quorum Ack
  -> Commit -> Runtime Response -> Client
```

------------------------------------------------------------------------

# 15. Governance

Architecture evolution SHALL follow:

1.  SAS
2.  RFC
3.  ADR
4.  Implementation Specification
5.  Code

No implementation may bypass the specification process.

Change control requirements:

-   Each RFC SHALL include migration and rollback strategy.
-   ADR entries SHALL include rejected alternatives.
-   Backward compatibility impact SHALL be explicitly labeled.
-   Any proposal introducing non-Rust production components requires
      explicit architecture exception approval.

------------------------------------------------------------------------

# 16. Delivery Roadmap

1.  R1 MVP (Rust): RDF Runtime + SPARQL Query + Single-region Cluster
      Core
2.  R2 (Rust): Cost-based Optimizer + OWL 2 RL Core + SHACL Baseline
3.  R3 (Rust): Advanced Cluster Operations + GeoSPARQL + Enterprise
      Security
4.  R4 (Rust): AI-native Semantic Runtime capabilities

MVP non-goals (R1):

-   Multi-region active-active
-   Full OWL 2 profile coverage
-   Federated query across heterogeneous remote sources
-   Any non-Rust implementation in production execution path

------------------------------------------------------------------------

# 17. Architecture Risks and Mitigations

-   Risk: scope explosion across standards and distribution features
      Mitigation: strict R1 scope gate and deferred feature flags
-   Risk: plugin instability and ABI drift
      Mitigation: versioned plugin API plus compatibility CI matrix
-   Risk: cross-shard transaction complexity
      Mitigation: constrain workload routing in R1 and phase-in 2PC
-   Risk: reasoning cost impacts query latency
      Mitigation: configurable reasoning modes and pre-materialization
-   Risk: Rust talent and ecosystem learning curve
      Mitigation: internal Rust guidelines, crate templates, and staged
      onboarding plan

------------------------------------------------------------------------

# Motto

**Standards First.**

**Reasoning Native.**

**Cloud Native.**

**Rust Powered.**
