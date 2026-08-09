# Ontolith Architecture Handbook

**Version:** 1.0 (Approved)\
**Project:** Ontolith\
**Owner:** sharky-ai

## Overview

This handbook defines the complete architecture documentation structure
for Ontolith.

## Volume 00 -- Foundation

-   0000 Project Vision
-   0001 Design Principles
-   0002 Terminology
-   0003 Glossary
-   0004 Architecture Philosophy
-   0005 Coding Principles
-   0006 Naming Convention
-   0007 Compatibility Policy
-   0008 Versioning Policy
-   0009 Deprecation Policy
-   0010 Governance

## Volume 01 -- Overall Architecture

-   0100 System Overview
-   0101 Context Diagram
-   0102 Layered Architecture
-   0103 Component Model
-   0104 Runtime Model
-   0105 Deployment Model
-   0106 Thread Model
-   0107 Memory Model
-   0108 Async Model
-   0109 Failure Model
-   0110 Module Dependency
-   0111 Extension Points
-   0112 Configuration
-   0113 Lifecycle
-   0114 Metrics
-   0115 Logging
-   0116 Tracing

## Volume 02 -- RDF Runtime

-   RDF Overview
-   Node
-   IRI
-   Blank Node
-   Literal
-   Datatype
-   Language Tag
-   Triple
-   Quad
-   Graph
-   Dataset
-   Namespace
-   Prefix
-   Vocabulary
-   RDF Collections
-   RDF-star
-   Serialization
-   Canonicalization
-   Hashing
-   Equality

## Volume 03 -- Parser

-   Lexer
-   Parser
-   AST
-   Error Recovery
-   Streaming Parser
-   Turtle
-   RDF/XML
-   JSON-LD
-   N-Triples
-   N-Quads
-   TriG
-   RDF-star
-   Parser Plugins

## Volume 04 -- Storage Engine

-   Storage Overview
-   Dictionary
-   Triple Encoding
-   Page Layout
-   File Format
-   WAL
-   MVCC
-   Snapshot
-   Transaction
-   Index Manager
-   Cache
-   Compression
-   Recovery
-   Checksum
-   Statistics
-   Storage API
-   RocksDB Backend
-   Memory Backend
-   Distributed Storage

## Volume 05 -- Query Engine

-   Grammar
-   AST
-   Algebra
-   Logical Plan
-   Physical Plan
-   Cost Model
-   Optimizer
-   Volcano Iterator
-   Executor
-   Hash Join
-   Merge Join
-   Nested Loop
-   Property Path
-   Federation
-   Streaming
-   Prepared Query

## Volume 06 -- Reasoning

-   Ontology Loader
-   Ontology Registry
-   TBox
-   ABox
-   RDFS
-   OWL RL
-   OWL DL
-   Rule Engine
-   SWRL
-   Truth Maintenance
-   Incremental Reasoning
-   Materialization
-   Query-time Reasoning
-   Hybrid Reasoning

## Volume 07 -- SHACL

-   Shapes
-   Node Shapes
-   Property Shapes
-   Validation
-   Constraint Engine
-   SHACL-SPARQL
-   Reports

## Volume 08 -- Cluster

-   Metadata
-   Raft
-   Placement
-   Sharding
-   Replica
-   Failover
-   Rebalancing
-   Scheduler
-   Membership
-   Gossip
-   Snapshot
-   Backup

## Volume 09 -- API

-   Rust API
-   REST
-   gRPC
-   SPARQL Protocol
-   WebSocket
-   SDK
-   Java
-   Python
-   Go
-   TypeScript

## Volume 10 -- Plugin System

-   Plugin Manager
-   Storage Plugin
-   Parser Plugin
-   Serializer Plugin
-   Reasoner Plugin
-   Optimizer Plugin
-   Security Plugin
-   Vector Plugin
-   Full Text Plugin

## Volume 11 -- Security

-   Authentication
-   Authorization
-   RBAC
-   ABAC
-   OAuth2
-   OIDC
-   TLS
-   Encryption
-   Audit

## Volume 12 -- Performance

-   Benchmarks
-   Memory
-   CPU
-   IO
-   SIMD
-   Parallelism
-   Async
-   Cache
-   Compression
-   Profiling

## Volume 13 -- Observability

-   Metrics
-   Logging
-   Tracing
-   Health Check
-   Alerting
-   Dashboards

## Volume 14 -- Testing

-   Unit
-   Integration
-   Compliance
-   Performance
-   Chaos
-   Fault Injection
-   Compatibility

## Volume 15 -- Compliance

-   RDF 1.2
-   RDF-star
-   SPARQL 1.1
-   SHACL
-   OWL RL
-   GeoSPARQL
-   SKOS
-   PROV-O
-   JSON-LD
-   RDF/XML

## Volume 16 -- Operations

-   Deployment
-   Kubernetes
-   Upgrade
-   Backup
-   Restore
-   Disaster Recovery
-   Monitoring

## Volume 17 -- AI Integration

-   Semantic Agent
-   MCP
-   RAG
-   Vector Bridge
-   Hybrid Search
-   LLM Plugin
-   Semantic Planning
-   AI Reasoning Integration

## Volume 18 -- Developer Handbook

-   Build
-   Code Style
-   Unsafe Policy
-   Benchmark
-   Release
-   Contribution
-   RFC
-   ADR

## Estimated Scale

  Item                                 Estimate
  -------------------- ------------------------
  Volumes                                    19
  Chapters                                \~220
  Markdown Documents                   220--300
  RFCs                                 150--250
  ADRs                                  80--120
  Compliance Items                        3000+
  Total Size             0.8--1.5 million words

## Current Document Coverage（2026-08-09 定稿快照）

按 “Specification Before Implementation” 原则，以下现有文档已映射到各卷/章
（定稿状态以各文档头部为准；未覆盖章节为后续实现前必须补规格的待办区）：

| 卷 | 覆盖文档 |
|----|----------|
| Volume 00 基础 | [Ontolith_Architecture_Handbook_Table_of_Contents.md](./Ontolith_Architecture_Handbook_Table_of_Contents.md)（0000–0010 治理框架） |
| Volume 01 总体架构 | [Ontolith_Software_Architecture_Specification.md](./Ontolith_Software_Architecture_Specification.md)（SAS-0001 1.2.0 Approved：原则/标准/布局/治理/路线/风险） |
| Volume 02 RDF 运行时 | [SAS-0401 — Knowledge Object Model.md](./SAS-0401%20—%20Knowledge%20Object%20Model.md)（1.0.0 Approved）+ [L1](./L1-ontolith-rdf-Statement-Graph-Dataset.md) + [L0](./L0-ontolith-core-Knowledge-Object-Foundation.md) |
| Volume 03 Parser | [L3](./L3-ontolith-parser-query.md)（Turtle 文法/流式解析/错误契约） |
| Volume 04 存储引擎 | [Ontolith Software Architecture Specification  Volume 04.md](./Ontolith%20Software%20Architecture%20Specification%20%20Volume%2004.md)（SAS-0400 1.0.0 Approved）+ [L2](./L2-ontolith-storage-transaction-kernel.md) + [L2-storage-contracts.md](./L2-storage-contracts.md) + [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)（磁盘布局/键格式） |
| Volume 05 查询引擎 | [L3](./L3-ontolith-parser-query.md)（代数/优化/聚合/Update）+ [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md) |
| Volume 06 推理 | [L6 推理与验证](../docs/PROGRESS.md)（OWL 2 RL 规则集 + 护栏） |
| Volume 07 SHACL | [L6 推理与验证](../docs/PROGRESS.md)（SHACL 98/98）+ `ontolith-reasoner` SHACL 引擎 |
| Volume 08 集群 | [L4](./L4-ontolith-cluster-consistency.md) + [ADR-0002](../adr/0002-cluster-mvp-in-process.md) + [ADR-0004](../adr/0004-multi-process-raft-data-plane.md)（openraft 数据面） |
| Volume 09 API | [L5](./L5-ontolith-access-security.md)（REST）+ gRPC 网关 + [L8](./L8-ai-native.md)（semantic API） |
| Volume 10 插件系统 | [L8](./L8-ai-native.md) §8（AgentTool 契约）+ `ontolith-plugin-api`（capabilities） |
| Volume 11 安全 | [L5](./L5-ontolith-access-security.md) + [L5-management-platform-slo.md](./L5-management-platform-slo.md) + [ADR-0003](../adr/0003-management-plane-security-minimum.md)（TLS/JWT/OIDC/审计） |
| Volume 12 性能 | [benchmarks/README.md](../benchmarks/README.md)（storage/semantic bench + 阈值/趋势） |
| Volume 13 可观测性 | [L5 可观测性](../docs/PROGRESS.md)（tracing/metrics/SLO） |
| Volume 14 测试 | `ontolith-compliance`（W3C/SHACL/R2/P8-02 gates）+ [ci-local](../scripts/ci-local.sh) |
| Volume 15 合规 | [PROGRESS.md](./PROGRESS.md)（W3C 492/492 + SHACL 98/98 profile 基线） |
| Volume 16 运维 | [L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md) + [L7-release-rollback.md](./L7-release-rollback.md) + systemd 部署脚本 |
| Volume 17 AI 集成 | [L8-ai-native.md](./L8-ai-native.md)（0.1.4 Active：语义检索 + AgentTool 扩展点，R4 立项文档） |
| Volume 18 开发者手册 | 本仓库（构建/风格/unsafe 策略/bench/发布/RFC/ADR 均已有文档或模板） |

## Core Principle

**Specification Before Implementation**

All production code must trace back to an approved SAS, RFC, or ADR.
