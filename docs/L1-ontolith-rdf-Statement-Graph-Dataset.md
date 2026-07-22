# L1 — ontolith-rdf Statement / Graph / Dataset 功能说明

文档 ID: IMPL-L1-0001  
版本: 1.0.0  
状态: Implemented  
日期: 2026-07-17  
对应 crate: `crates/ontolith-rdf`  
规范依据:

- [SAS-0001 Software Architecture Specification](./Ontolith_Software_Architecture_Specification.md)
- [SAS-0401 Knowledge Object Model](./SAS-0401%20—%20Knowledge%20Object%20Model.md)
- [L0 ontolith-core 功能说明](./L0-ontolith-core-Knowledge-Object-Foundation.md)
- [PLAN-0001 / Phase 1](./Ontolith_Development_Plan.zh-CN.md)

---

## 1. 层定位

### 1.1 架构位置

```text
L3+ query / parser / reasoner / server
L2  ontolith-storage / ontolith-transaction
L1  ontolith-rdf          ← 本层：Term / Triple / Quad / Graph / Dataset
L0  ontolith-core         身份、Resource、KO Header、Canonical、Error
```

### 1.2 职责

| 负责 | 不负责 |
|------|--------|
| RDF 语句值类型（Triple/Quad） | 语法解析（Turtle/N-Triples 等 → `ontolith-parser`） |
| Named Graph / Dataset 内存模型 | 持久化、WAL、索引（→ `ontolith-storage`） |
| Statement 级 canonical 编码 | SPARQL 执行 |
| 与 L0 `DatasetObject` / `GraphObject` 桥接 | 推理、SHACL |
| 轻量 `DatasetService` 纯变换 | 网络 / 文件 IO |

### 1.3 依赖

- **依赖**：`ontolith-core` 仅  
- **被依赖**：`ontolith-storage`、`ontolith-parser`、`ontolith-query`（测试）、`ontolith-sdk`、`ontolith-observability`（测试）等  

---

## 2. 模块结构

```text
crates/ontolith-rdf/src/
├── lib.rs                 # LAYER = L1-rdf-statement-model
├── domain/
│   ├── mod.rs             # 导出 + 单测
│   ├── term.rs            # Term / Subject / Predicate
│   ├── statement.rs       # Triple / Quad / StatementObject
│   └── graph.rs           # NamedGraph / Dataset + KO 桥接
├── application/mod.rs     # DatasetService
└── infrastructure/mod.rs  # 占位（解析器不在本层）
```

---

## 3. 功能清单

### 3.1 Term 与语句位置（`domain::term`）

| 类型 | 说明 |
|------|------|
| `Term` | 对象位：`Iri` / `BlankNode(NodeId)` / `Literal(LiteralValue)` |
| `Subject` | 主语位新类型包装 `NodeId`（不可为字面量） |
| `Predicate` | 谓语位新类型包装 `Iri`；`parse` 走 L0 IRI 校验 |

兼容约定（**存储热路径保持不变**）：

- `Triple.subject: NodeId`
- `Triple.predicate: Iri`
- `Triple.object: Term`

`Term::to_resource()` 可投影到 L0 `Resource`（blank 仅有 NodeId 时合成标签 `n{id}`）。

### 3.2 Statement（`domain::statement`）

| 类型 | 说明 |
|------|------|
| `Triple` | 不可变三元组；`new` / `validated` / `with_graph` |
| `Quad` | `triple` + `graph_name: Option<Iri>`（`None` = 默认图） |
| `StatementObject` | KO 包装：`KnowledgeObjectHeader` + `Quad`（目录/审计用，非热路径） |

校验（R1）：

- `Triple::validated`：predicate 必须通过 `Iri::parse`
- `Quad::validated`：triple 校验 + 命名图名 IRI 校验

图身份：

- `Quad::graph_id() -> GraphId`（L0）
- `in_default_graph` / `in_named_graph` 构造器

Canonical：

- Triple tag `T3` + subject u64 + predicate str + object term  
- Quad tag `T4` + `GD`|`GN` + triple  

### 3.3 Graph / Dataset（`domain::graph`）

| 类型 | 说明 |
|------|------|
| `NamedGraph` | `name: Iri` + `triples: Vec<Triple>` |
| `Dataset` | `default_graph` + `named_graphs`（导入/导出逻辑边界） |

Dataset API：

| 方法 | 行为 |
|------|------|
| `insert_default` / `insert_named` / `insert_quad` | 追加写入（当前 **不去重**） |
| `named_graph` / `named_graph_mut` | 按名查找 |
| `graph_count` / `triple_count` / `quads` | 聚合视图 |
| `default_statistics` / `NamedGraph::statistics` | 基数统计 |
| `to_dataset_object` | 桥接 L0 `DatasetObject`（header + stats，不含 triple 载荷） |
| `merge` | 合并另一 dataset（追加） |
| `is_empty` | 是否无三元组 |

Canonical：

- Dataset 默认图三元组按 canonical 字节排序后编码  
- 命名图按图名排序，图内三元组同样排序  
- 因此 **插入顺序不影响** dataset canonical 字节  

统计：

- `triple_count`、`distinct_subjects`、`distinct_predicates`、`distinct_objects`  
- object 去重基于 `Term` canonical 字节  

### 3.4 Application（`application::DatasetService`）

纯函数式辅助，无 IO：

- `empty` / `from_triples` / `from_quads`
- `to_knowledge_object`
- `merge`

### 3.5 Crate 元信息

| 项 | 值 |
|----|-----|
| `CRATE_ID` | `"ontolith-rdf"` |
| `LAYER` | `"L1-rdf-statement-model"` |
| `healthcheck()` | `true` |

---

## 4. 与 L0 的边界

| L0 类型 | L1 用法 |
|---------|---------|
| `NodeId` / `Iri` / `LiteralValue` | Triple 字段与 Term |
| `GraphId` / `GraphObject` / `DatasetObject` | Quad 图身份；`to_dataset_object` 桥接 |
| `KnowledgeObjectHeader` / `ObjectType::Statement` | `StatementObject` |
| `CanonicalEncode` | Term/Triple/Quad/NamedGraph/Dataset |
| `OntolithError` | 校验失败 |

L1 **不** 重新定义 `Iri`/`NodeId`/`ObjectId`。

---

## 5. 不变量

1. Statement 值对象在创建后字段不提供内部可变 API（容器 `Dataset` 可追加）。  
2. 默认图用 `graph_name = None` / `GraphId::Default` 表达，不用魔法 IRI。  
3. Canonical 对多重集语义：相同三元组多重出现会保留（排序稳定，但未 set 去重）。  
4. 热路径类型字段名与布局保持兼容，避免破坏 `ontolith-storage`。  
5. 不引入第三方依赖。  

---

## 6. 测试与验收

### 6.1 单测

| 用例 | 覆盖 |
|------|------|
| `triple_canonical_is_stable_and_order_sensitive` | 编码稳定 |
| `triple_validation_rejects_bad_predicate` | 校验 |
| `quad_default_and_named_graph_ids` | 图身份 |
| `dataset_insert_and_quad_roundtrip` | 写入与 quads |
| `dataset_canonical_ignores_triple_insertion_order` | 默认图顺序无关 |
| `dataset_canonical_sorts_named_graphs_by_name` | 命名图顺序无关 |
| `dataset_bridges_to_core_dataset_object` | L0 桥接 |
| `statement_object_wraps_triple` | KO 包装 |
| `term_kinds_and_resource_projection` | Term 投影 |
| `statistics_count_distincts` | 统计 |
| `service_builds_dataset_from_quads` | DatasetService |

### 6.2 回归

下游 `storage` / `query` / `observability` / `parser` / `sdk` 编译与既有测试保持绿色。

### 6.3 L1 Done 标准

- [x] Triple/Quad 完整且可校验  
- [x] Dataset 为导入导出边界  
- [x] Canonical 确定性（插入顺序无关）  
- [x] 桥接 L0 DatasetObject  
- [x] 兼容既有存储字段布局  
- [x] 单测 + 下游回归通过  

---

## 7. 已知限制与后续

| 项 | 现状 | 后续 |
|----|------|------|
| 去重 | 追加型多重集 | 可选 set-semantics API |
| RDF-star | 未支持 | 标准增强 |
| 主语为 IRI 文本 | 仅 NodeId | 字典层统一编码 |
| 字面量完整类型 | 复用 L0 `LiteralValue` | 热路径升级到 `Literal` |
| 语法解析 | 无 | L3 `ontolith-parser` |
| 物理编码 / 索引键 | 无 | L2 storage |

---

## 8. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | 首版 L1 实现与功能说明 |

---

## 9. 代码索引

| 主题 | 路径 |
|------|------|
| Term | `crates/ontolith-rdf/src/domain/term.rs` |
| Triple/Quad | `crates/ontolith-rdf/src/domain/statement.rs` |
| Graph/Dataset | `crates/ontolith-rdf/src/domain/graph.rs` |
| DatasetService | `crates/ontolith-rdf/src/application/mod.rs` |
| 单测 | `crates/ontolith-rdf/src/domain/mod.rs`、`application/mod.rs` |
