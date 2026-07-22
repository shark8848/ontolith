# Ontolith Development Plan

Document ID: PLAN-0001  
Version: 1.0.0-draft  
Status: Draft  
Date: 2026-07-12  
Owner: sharky-ai

Progress ledger (execution status): [PROGRESS.md](./PROGRESS.md) (PROG-0001)  
Layer implementation notes: [L0 core](./L0-ontolith-core-Knowledge-Object-Foundation.md) · [L1 rdf](./L1-ontolith-rdf-Statement-Graph-Dataset.md) · [L2 storage/txn](./L2-ontolith-storage-transaction-kernel.md) · [L3 parser/query](./L3-ontolith-parser-query.md)

---

## 1. Objective

This plan defines a complete implementation path for Ontolith from R1 to R4, aligned with the architecture specification, Rust-only production policy, and controlled third-party reuse policy.

The plan is organized in two views:
- Review View: scope, decisions, constraints, and risks.
- Execution View: work breakdown, dependencies, milestones, acceptance, and delivery cadence.

---

## 2. Inputs and References

- Ontolith_Software_Architecture_Specification.md
- Ontolith Software Architecture Specification  Volume 04.md
- SAS-0401 - Knowledge Object Model.md
- Ontolith_Architecture_Handbook_Table_of_Contents.md

---

## 3. Review View

### 3.1 Scope

Included:
- End-to-end technical delivery from R1 to R4.
- Runtime architecture implementation and engineering process.
- Security, observability, release, and dependency governance.
- Third-party component control for Oxigraph, RocksDB, and core Rust ecosystem crates.

Excluded:
- Commercial pricing and go-to-market strategy.
- Non-technical organizational performance policy.
- Standalone UI product design details.

### 3.2 Core Decisions

- Production path is Rust-only for control plane and data plane.
- Third-party native engines are allowed only behind Rust adapter boundaries.
- Oxigraph is used as a reusable semantic kernel candidate, not as a platform boundary.
- RocksDB is used through storage abstraction and must not leak vendor APIs to upper layers.
- Standards compliance is mandatory for RDF/SPARQL behavior in release gates.

### 3.3 Architecture Constraints

- Internal abstractions are mandatory for storage, query, and plugin boundaries.
- Deterministic behavior is required for unsupported standards features (documented error semantics).
- Consistency levels and transaction semantics must be explicit per API.
- RFC and ADR are mandatory for Tier A dependency introduction or architecture exceptions.

### 3.4 Primary Risks and Mitigations

- Scope expansion risk: enforce R1 non-goals and feature flags.
- Distributed complexity risk: single-region baseline first, then advanced cluster features.
- Reasoning cost risk: configurable reasoning modes, avoid deep reasoning in hot query path.
- Dependency lock-in risk: strict interface isolation and compatibility matrix.
- Team ramp-up risk: Rust guidelines, templates, and staged onboarding.

---

## 4. Execution View

### 4.1 Delivery Phases

#### Phase 0 - Planning and Governance Baseline

Goals:
- Freeze scope, terminology, milestones, and non-goals.
- Establish RFC/ADR workflow and dependency intake process.

Deliverables:
- Approved scope baseline.
- Architecture exception template.
- Dependency registry template and review policy.

#### Phase 1 - Core Model and Storage Abstraction

Goals:
- Implement Knowledge Object domain model.
- Implement node identifiers and dictionary manager.
- Define storage abstraction interfaces.

Deliverables:
- ontolith-core model crate baseline.
- Dictionary service with bidirectional mapping.
- Stable storage trait contracts.

Dependencies:
- Blocks Phases 2 and 3.

#### Phase 2 - Persistence and Transaction Kernel

Goals:
- Implement RocksDB adapter under storage abstraction.
- Implement WAL, snapshot recovery, MVCC baseline.
- Implement triple/quad encoding and foundational indexes.

Deliverables:
- Durable write path with recovery checks.
- Index set baseline (SPO/POS/OSP at minimum).
- Transaction behavior specification.

Dependencies:
- Depends on Phase 1.

#### Phase 3 - Query Engine MVP

Goals:
- Implement SPARQL parse-to-execution primary pipeline.
- Implement rule-based optimization baseline.
- Provide explain plan, timeout, and cancellation.

Deliverables:
- Query execution pipeline for MVP profile.
- Logical and physical explain outputs.
- Timeout and cancellation APIs.

Dependencies:
- Depends on Phase 2.
- Can run in parallel with Phase 4.

#### Phase 4 - Cluster and Consistency MVP

Goals:
- Implement metadata service and leader election.
- Implement single-region sharding and replication baseline.
- Implement consistency levels for client reads.

Deliverables:
- Raft-based metadata control baseline.
- Replication and failover baseline.
- API-level consistency behavior document.

Dependencies:
- Depends on Phase 2.
- Can run in parallel with Phase 3.

#### Phase 5 - Access Layer and Security Baseline

Goals:
- Implement gateway and service access boundaries.
- Implement authn/authz, tenant isolation, and audit logging.
- Implement observability baseline (metrics, traces, logs).

Deliverables:
- Security baseline in runtime path.
- Tenant-safe request handling baseline.
- Unified telemetry baseline.

Dependencies:
- Depends on Phases 3 and 4.

#### Phase 6 - Reasoning and Validation Enhancement

Goals:
- Implement OWL 2 RL core rule set.
- Implement SHACL baseline validator.
- Implement configurable reasoning modes with safeguards.

Deliverables:
- Reasoning baseline profile.
- SHACL baseline report semantics.
- Performance guardrails for reasoning paths.

Dependencies:
- Depends on Phases 3 and 5.

#### Phase 7 - Enterprise Operations and Release Engineering

Goals:
- Implement online rebalancing and DR rehearsal workflows.
- Implement performance regression gates and SLO dashboards.
- Implement release pipeline with rollback validation.

Deliverables:
- Operational readiness baseline.
- DR drill runbooks and evidence.
- Release and rollback playbooks.

Dependencies:
- Depends on Phases 5 and 6.

#### Phase 8 - AI-Native Semantic Extensions

Goals:
- Implement semantic-vector bridge capabilities.
- Implement retrieval augmentation interfaces.
- Implement agent integration extension points.

Deliverables:
- AI-native extension baseline.
- Compatibility and safety guardrails for AI paths.

Dependencies:
- Depends on Phase 7.

---

## 5. Work Breakdown Structure (WBS)

### WBS-01 Core Runtime and Knowledge Model
- Resource, Statement, Graph, Dataset, Ontology structures.
- Deterministic identity and canonical encoding rules.

### WBS-02 Parser and Ingest
- RDF syntax support for MVP profile.
- Stream-safe parser pipeline and error contracts.

### WBS-03 Storage and Transaction
- Dictionary manager.
- Triple/quad physical encoding.
- WAL, MVCC, snapshot, and recovery.

### WBS-04 Query and Optimization
- SPARQL parser and algebra translation.
- Logical/physical planner and iterator executor.
- Explain, timeout, and cancellation.

### WBS-05 Reasoning and SHACL
- OWL 2 RL core rules.
- SHACL baseline constraints and reports.

### WBS-06 Distributed Runtime
- Metadata service and consensus.
- Sharding, replication, failover, and rebalancing.

### WBS-07 API, Security, and Integrations
- API gateway boundaries.
- Authn/authz, tenant isolation, audit trail.
- Plugin host and extension lifecycle.

### WBS-08 Platform Engineering
- CI/CD, quality gates, dependency audit, release process.
- Observability stack and operations runbooks.

---

## 6. Milestones (R1 to R4)

### R1 MVP (Rust)
Scope:
- Core RDF runtime.
- SPARQL query baseline.
- Single-region cluster core.
- Security and audit baseline.

Exit criteria:
- Standards conformance gate passes for MVP scope.
- Core SLO baseline achieved.
- Recovery drill and rollback drill both pass.

### R2 (Rust)
Scope:
- Cost-based optimization.
- OWL 2 RL core.
- SHACL baseline.

Exit criteria:
- Explain plan quality and optimization stability gates pass.
- Reasoning correctness and performance guardrails pass.

### R3 (Rust)
Scope:
- Advanced cluster operations.
- GeoSPARQL scope.
- Enterprise security hardening.

Exit criteria:
- High-availability and failover gates pass.
- Tenant isolation and audit hardening checks pass.

### R4 (Rust)
Scope:
- AI-native semantic runtime extensions.

Exit criteria:
- Extension safety gates and compatibility gates pass.
- Retrieval and semantic integration KPIs meet target.

---

## 7. Acceptance and Quality Gates

### 7.1 Functional
- RDF/SPARQL conformance tests for supported profile must pass.
- API contracts and error semantics must be deterministic.

### 7.2 Consistency and Reliability
- Fault-injection tests must validate election, replication, and recovery behavior.
- Idempotent write behavior must be validated under retries.

### 7.3 Performance and Capacity
- Latency, throughput, RTO, and RPO must meet release targets.
- Performance regression threshold breaches must block release candidates.

### 7.4 Security and Compliance
- Authz policy tests and tenant isolation tests must pass.
- Dependency license checks and vulnerability checks must pass.

### 7.5 Engineering Quality
- cargo fmt --check, cargo clippy -D warnings, and full tests must pass.
- Safety-sensitive modules should run Miri/sanitizer checks in CI.

---

## 8. Third-Party Dependency Governance Plan

### 8.1 Tiering
- Tier A: runtime-critical dependencies.
- Tier B: runtime-optional dependencies with degradation path.
- Tier C: build, test, and developer-only dependencies.

### 8.2 Initial Baseline
- Oxigraph: semantic kernel reuse candidate.
- RocksDB (via Rust bindings): durability and storage candidate.
- Tokio, Serde, Tracing ecosystems: runtime and platform baseline.

### 8.3 Approval Rules
- New Tier A dependency requires RFC and ADR approval.
- Each dependency must define owner, risk level, fallback, and replacement path.
- Vendor-specific APIs must not leak above internal abstraction boundaries.

### 8.4 Version and Supply Chain Policy
- No wildcard versions for production dependencies.
- Cargo.lock committed for reproducible builds.
- Continuous CVE and dependency audit in CI.
- Critical vulnerability remediation target: 72 hours.

---

## 9. Team and Operating Model

### 9.1 Suggested Streams
- Stream A: Core storage and transaction.
- Stream B: Query and reasoning.
- Stream C: Distributed runtime.
- Stream D: Platform security and operations.

### 9.2 Cadence
- R1 target: 2 to 3 months for MVP.
- R2 and R3: quarterly increments with hard release gates.
- Reserve 15 to 20 percent capacity each phase for stability and regression work.

---

## 10. Reporting and Artifacts

Mandatory artifacts per phase:
- Design package: interfaces, constraints, ADR links.
- Validation package: test reports and benchmark reports.
- Operations package: runbooks, SLO dashboard snapshots, rollback evidence.
- Governance package: dependency registry updates and approval records.

---

## 11. Immediate Next Actions

- Confirm owners for each stream.
- Approve Phase 0 deliverables and due dates.
- Start Phase 1 implementation with weekly architecture and risk review.
