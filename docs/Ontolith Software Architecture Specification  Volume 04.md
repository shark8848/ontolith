# SAS-0400

# Knowledge Data Engine Architecture Overview

---

Document Information

| Item | Value |
|------|-------|
| Document ID | SAS-0400 |
| Title | Knowledge Data Engine Architecture Overview |
| Project | Ontolith |
| Version | 1.0.0 |
| Status | Approved |
| Owner | sharky-ai |
| Architecture Board | Ontolith Architecture Working Group |
| Classification | Normative |
| Last Updated | 2026-07-12 |

---

# Abstract

The Knowledge Data Engine (KDE) is the core data management subsystem of Ontolith.

It provides the logical and physical management of RDF datasets, ontology metadata, semantic graphs, transactions, indexing, versioning, and distributed persistence.

Unlike traditional RDF triple stores, the Knowledge Data Engine is designed as a cloud-native, distributed, highly available semantic data platform capable of supporting enterprise-scale ontology reasoning.

The Knowledge Data Engine SHALL provide deterministic behavior independent of storage backend implementation.

---

# Purpose

This specification defines the architecture of the Knowledge Data Engine.

It specifies

- responsibilities
- architectural boundaries
- component interactions
- storage abstraction
- transaction model
- distributed integration
- backend interfaces

This specification is normative.

All implementations SHALL conform to this document.

---

# Scope

This document covers

- RDF data storage
- Graph storage
- Dataset storage
- Metadata storage
- Dictionary encoding
- Triple management
- Quad management
- Index management
- Transaction management
- Version management
- Distributed storage integration
- Storage backend abstraction

This document does not define

- SPARQL grammar
- OWL semantics
- SHACL validation
- Query optimization

Those are defined in other specifications.

---

# Architecture Position

The Knowledge Data Engine is located between the Semantic Runtime and the Storage Backend.

```text
Applications
        │
Semantic Runtime
        │
Reasoning Runtime
        │
Knowledge Data Engine
        │
Distributed Runtime
        │
Storage Backend
        │
Persistent Storage
```

The Knowledge Data Engine SHALL NOT expose storage implementation details to upper layers.

---

# Design Objectives

The Knowledge Data Engine SHALL satisfy the following objectives.

## O-001

Storage independence.

Upper layers SHALL NOT depend on RocksDB or any specific storage implementation.

---

## O-002

Semantic independence.

Storage backend SHALL NOT understand RDF semantics.

---

## O-003

Deterministic storage.

Equivalent datasets SHALL produce identical internal representations.

---

## O-004

Distributed by design.

Every component SHALL support future distributed execution.

---

## O-005

High availability.

Failure of a single storage node SHALL NOT interrupt service.

---

## O-006

Scalability.

Horizontal scaling SHALL be supported without application changes.

---

## O-007

Plugin architecture.

Storage implementations SHALL be replaceable.

---

# Architectural Principles

The Knowledge Data Engine follows the following principles.

## Separation of Concerns

Semantic processing SHALL be separated from physical persistence.

---

## Logical Independence

Knowledge objects SHALL be represented independently of storage technology.

---

## Storage Abstraction

All storage SHALL be accessed through a backend abstraction layer.

---

## Immutable Data Model

Persistent records SHOULD be append-only whenever practical.

---

## MVCC

Concurrent readers SHALL never block writers.

---

## Version First

Every semantic object MAY be versioned.

---

## Distributed First

Cluster deployment SHALL be considered the primary deployment model.

---

# Core Responsibilities

The Knowledge Data Engine SHALL provide the following services.

| Service | Responsibility |
|----------|----------------|
| Metadata Manager | Database metadata |
| Dictionary Manager | RDF node encoding |
| Node Manager | Node lifecycle |
| Triple Store | Triple persistence |
| Quad Store | Named graph persistence |
| Graph Manager | Graph lifecycle |
| Dataset Manager | Dataset lifecycle |
| Index Manager | Multi-index management |
| Transaction Manager | MVCC |
| Version Manager | Object versioning |
| Statistics Manager | Query statistics |
| Cache Manager | In-memory caching |
| WAL Manager | Write-ahead logging |
| Recovery Manager | Crash recovery |
| Storage Adapter | Backend abstraction |

---

# High-Level Architecture

```text
                Knowledge Data Engine
                        │
 ┌────────────────────────────────────────────────┐
 │ Metadata Manager                               │
 │ Dictionary Manager                             │
 │ Node Manager                                   │
 │ Triple Store                                   │
 │ Quad Store                                     │
 │ Graph Manager                                  │
 │ Dataset Manager                                │
 │ Index Manager                                  │
 │ Transaction Manager                            │
 │ Version Manager                                │
 │ Statistics Manager                             │
 │ Cache Manager                                  │
 │ WAL Manager                                    │
 │ Recovery Manager                               │
 └────────────────────────────────────────────────┘
                        │
                Storage Adapter
                        │
      ┌─────────────────┼─────────────────┐
      │                 │                 │
   RocksDB          TiKV          FoundationDB
```

---

# Non-Goals

The Knowledge Data Engine SHALL NOT

- implement SPARQL parsing
- execute OWL reasoning
- evaluate SHACL rules
- expose physical storage layout
- depend on any specific storage backend

---

# Storage Philosophy

The Knowledge Data Engine stores **knowledge**, not files.

Persistent storage backends store **bytes**, not RDF.

This distinction is fundamental.

The engine owns the logical data model.

The backend owns persistence.

---

# Backend Independence

The following storage backends SHOULD be supported.

- RocksDB
- TiKV
- FoundationDB
- In-Memory
- Object Storage

Additional backends MAY be implemented through the Storage Backend API.

---

# Distributed Deployment

The Knowledge Data Engine SHALL support

- multi-node deployment
- automatic replication
- distributed transactions
- shard migration
- online scaling
- automatic failover
- metadata synchronization

Distributed execution SHALL remain transparent to applications.

---

# Compliance

Every implementation SHALL satisfy

- SAS-0400
- Storage Backend Specification
- Transaction Specification
- Distributed Runtime Specification

---

# References

Normative

- IEEE 42010
- RFC 2119
- RDF 1.2
- SPARQL 1.1

Informative

- PostgreSQL
- TiDB
- FoundationDB
- RocksDB
- Apache Jena

---

# Revision History

| Version | Date | Description |
|----------|------|-------------|
| 1.0.0-draft | 2026-07-12 | Initial architecture specification |
| 1.0.0 | 2026-08-09 | 评审定稿（Approved）：storage abstraction（内存/RocksDB 双后端 + MVCC 版本链 + 纯 CF 索引 + WAL 重放重建）、backend independence、distributed deployment（多进程 Raft + 快照迁移）逐项对照实现核验一致；评审人 sharky-ai（Codex 执行体代执行），评审记录见上表下附注 |


### 评审记录（Review Record）

| 项 | 值 |
|----|----|
| 评审日期 | 2026-08-09 |
| 评审人 | sharky-ai（项目负责人；依用户委托授权，Codex 执行体代为执行评审流程） |
| 范围 | 知识数据引擎总体架构：核心职责（存储抽象/事务/索引）、MVCC、版本优先、分布式优先、后端独立（内存引擎 ↔ RocksDB 后端一致性）、RocksDB 列族布局（data/index/raft/semantic CF）、合规基线 |
| 结论 | 通过：SAS-0400 的架构条款与 `ontolith-storage`/`ontolith-cluster` 实现一致（storage 53 测 + cluster 31 测 + 灾备演练 DRILL PASS 为证据） |

---

Next Document

SAS-0401

Knowledge Data Model