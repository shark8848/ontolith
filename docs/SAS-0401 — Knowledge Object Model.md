# SAS-0401

# Knowledge Object Model

---

## Document Information

| Item | Value |
|------|-------|
| Document ID | SAS-0401 |
| Title | Knowledge Object Model |
| Project | Ontolith |
| Version | 1.0.0-draft |
| Status | Draft |
| Classification | Normative |

---

# 1. Purpose

This specification defines the canonical knowledge object model of Ontolith.

All semantic information managed by Ontolith SHALL be represented as one or more Knowledge Objects.

The Knowledge Object Model provides a stable logical representation independent of:

- Storage Backend
- Query Engine
- Reasoning Engine
- Network Topology
- Serialization Format

---

# 2. Design Goals

The Knowledge Object Model SHALL satisfy the following goals.

## GO-001

Technology independence.

Knowledge Objects SHALL NOT depend on RocksDB, TiKV, FoundationDB or any storage implementation.

---

## GO-002

Semantic consistency.

Equivalent RDF datasets SHALL produce identical Knowledge Objects.

---

## GO-003

Stable identity.

Every Knowledge Object SHALL have a globally unique identifier.

---

## GO-004

Versionability.

Every persistent Knowledge Object MAY support version history.

---

## GO-005

Deterministic serialization.

Knowledge Objects SHALL support canonical serialization.

---

# 3. Knowledge Object Hierarchy

The Knowledge Object Model consists of the following object categories.

```text
Knowledge Object
│
├── Resource
│   ├── IRI
│   ├── Blank Node
│   └── Literal
│
├── Statement
│   ├── Triple
│   └── Quad
│
├── Graph
│
├── Dataset
│
├── Ontology
│
├── Rule
│
├── Version
│
└── Metadata
```

All persistent entities SHALL belong to exactly one primary category.

---

# 4. Base Knowledge Object

Every Knowledge Object SHALL inherit the following attributes.

| Attribute | Description |
|-----------|-------------|
| Object ID | Immutable unique identifier |
| Object Type | Runtime type |
| Version | Current version |
| Created At | Creation timestamp |
| Updated At | Last modification |
| State | Lifecycle state |

---

The conceptual model is:

```text
Knowledge Object
│
├── Object ID
├── Type
├── Version
├── Metadata
└── Payload
```

---

# 5. Resource Model

Resources represent RDF nodes.

Three resource types SHALL be supported.

| Resource | Description |
|----------|-------------|
| IRI | Global identifier |
| Blank Node | Anonymous resource |
| Literal | Typed value |

Every Resource SHALL have a Node Identifier.

Node Identifiers SHALL remain immutable throughout the lifetime of the database.

---

# 6. Statement Model

A Statement represents a semantic assertion.

Statements SHALL be immutable.

Triple Model

```text
Subject
Predicate
Object
```

Quad Model

```text
Graph
Subject
Predicate
Object
```

Statements SHALL reference Resources using Node Identifiers rather than textual values.

---

# 7. Graph Model

A Graph represents a collection of Statements.

Every Graph SHALL possess:

- Graph Identifier
- Graph Metadata
- Graph Statistics
- Graph Version

Graphs SHALL be independently managed and replicated.

---

# 8. Dataset Model

A Dataset is a collection of Graphs.

Datasets SHALL contain:

- Default Graph
- Zero or more Named Graphs

Datasets SHALL be the logical exchange boundary for import and export operations.

---

# 9. Ontology Model

An Ontology SHALL be represented as a specialized Dataset.

Ontologies MAY include:

- TBox
- ABox
- Annotation Graph
- Rule Graph
- Provenance Graph

Reasoning SHALL operate on Ontology objects rather than directly on storage records.

---

# 10. Object Relationships

The relationships between major objects are illustrated below.

```text
Dataset
│
├── Graph
│     │
│     ├── Triple
│     ├── Triple
│     └── Triple
│
└── Graph
      │
      └── Triple
```

Each Triple references Resource objects through immutable Node Identifiers.

---

# 11. Identity Model

Every persistent object SHALL possess:

- Logical Identifier
- Internal Identifier
- Version Identifier

Identifiers SHALL remain stable regardless of storage backend.

Logical identity SHALL be preserved across export, replication and migration.

---

# 12. Lifecycle

Knowledge Objects SHALL progress through the following lifecycle.

```text
Created
    │
Persisted
    │
Indexed
    │
Replicated
    │
Versioned
    │
Archived
    │
Deleted
```

Deletion SHOULD be logical whenever possible.

---

# Next Section

SAS-0401 Part II

Knowledge Object Serialization and Metadata Model