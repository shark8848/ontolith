# L0 — ontolith-core Knowledge Object 基座功能说明

文档 ID: IMPL-L0-0001  
版本: 1.0.0  
状态: Implemented  
日期: 2026-07-17  
对应 crate: `crates/ontolith-core`  
规范依据:

- [SAS-0001 Software Architecture Specification](./Ontolith_Software_Architecture_Specification.md)
- [SAS-0401 Knowledge Object Model](./SAS-0401%20—%20Knowledge%20Object%20Model.md)
- [PLAN-0001 / Phase 1](./Ontolith_Development_Plan.zh-CN.md)
- [PROG-0001 进度台账](./PROGRESS.md)

---

## 1. 层定位

### 1.1 在整体架构中的位置

```text
Applications / SDK
        │
API Gateway
        │
Semantic Runtime (query / reasoner / validation / txn)
        │
Storage Abstraction
        │
Distributed Runtime
        │
────────────────────────────────
L1  ontolith-rdf          Statement / Graph / Dataset 值类型
L0  ontolith-core         ← 本层：KO 身份、资源、生命周期、规范编码、错误
────────────────────────────────
```

`ontolith-core` 是 **最低共享领域层**。所有生产 crate 可依赖它；它 **不得** 依赖 `ontolith-rdf`、`ontolith-storage` 或任何上层 crate。

### 1.2 设计目标（映射 SAS-0401）

| 目标 | 说明 | 本层实现要点 |
|------|------|----------------|
| GO-001 技术无关 | 不依赖 RocksDB / 网络 / 查询引擎 | 纯领域类型 + 标准库 |
| GO-002 语义一致 | 等价数据集产生相同逻辑对象 | `CanonicalEncode` 基线 |
| GO-003 稳定身份 | 每个 KO 有全局唯一逻辑 ID | `ObjectId` |
| GO-004 可版本化 | 支持版本历史 | `ObjectVersion` / `VersionObject` |
| GO-005 确定性序列化 | 支持 canonical 序列化 | `CanonicalWriter` |

### 1.3 非目标（本层明确不做）

- RDF 语法解析 / 序列化文件格式（属 `ontolith-parser` / 后续序列化模块）
- Triple/Quad 存储与索引（属 `ontolith-storage`）
- SPARQL、推理、集群、鉴权
- 完整 RFC 3987 IRI 校验、RDF 1.2 全部字面量规则
- 持久化字典与物理编码（属 L2 存储）

---

## 2. 模块结构

```text
crates/ontolith-core/
├── Cargo.toml
└── src/
    ├── lib.rs                 # crate 入口，LAYER = L0-core-knowledge-object
    ├── error.rs               # 共享错误枚举
    ├── application/mod.rs     # 占位（本层无应用服务）
    ├── infrastructure/mod.rs  # 占位（本层无基础设施适配）
    └── domain/
        ├── mod.rs             # 导出 + NodeId + 单测
        ├── identity.rs        # 身份 / 类型 / 生命周期 / Header
        ├── resource.rs        # IRI / Blank / Literal / Resource
        ├── knowledge.rs       # Graph / Dataset / Ontology / Rule / Version
        └── canonical.rs       # 确定性编码 trait 与 writer
```

依赖：`ontolith-core` **无第三方依赖**（仅 Rust 标准库）。

---

## 3. 功能清单

### 3.1 身份模型（`domain::identity`）

| 类型 | 职责 | 约束 |
|------|------|------|
| `ObjectId` | KO 逻辑标识（不可变） | 非空 UTF-8；`new` 校验，`from_validated` 信任内部构造 |
| `ObjectVersion` | 对象内单调版本 | 默认初始 `INITIAL = 1`；`next()` 饱和递增 |
| `VersionId` | 谱系/快照用版本标识 | 新类型包装 `u64` |
| `ObjectType` | KO 运行时类别 | Resource / Statement / Graph / Dataset / Ontology / Rule / Version / Metadata |
| `ObjectState` | 生命周期状态 | Created → Persisted → Indexed → Replicated → Versioned；可 Archived；可逻辑 Deleted |
| `KnowledgeObjectHeader` | 每个 KO 必带公共头 | id, type, version, created_at, updated_at, state |

#### 生命周期迁移规则

允许：

| 从 | 到 |
|----|----|
| 任意非 Deleted | 同态自迁移（幂等） |
| Created | Persisted |
| Persisted | Indexed |
| Indexed | Replicated |
| Replicated | Versioned |
| Persisted / Indexed / Replicated / Versioned | Archived |
| Archived | Versioned |
| 任意非 Deleted | Deleted |

禁止：

- 从 `Deleted` 迁出
- 跳过中间主路径（例如 Created → Indexed）

API：

- `KnowledgeObjectHeader::new(id, object_type, created_at)`
- `touch(at)`：更新 `updated_at` 并递增 version
- `transition_to(next, at) -> Result<(), &'static str>`

### 3.2 资源模型（`domain::resource`）

对应 SAS-0401 §5：Resource = IRI | Blank Node | Literal。

| 类型 | 说明 |
|------|------|
| `Iri` | 字符串 IRI；`new` 不校验，`parse` 做 R1 基线校验 |
| `BlankNodeId` | 数据集局部空白节点标签；`parse` 要求非空且无空白 |
| `LanguageTag` | BCP 47 子集；规范化为小写 |
| `LiteralValue` | 紧凑字面量：String / Integer / Decimal(f64) / Boolean（兼容既有存储路径） |
| `Literal` | 完整字面量：value + datatype IRI + optional language |
| `Resource` | 三选一枚举 |
| `BoundResource` | `NodeId` + `Resource`（字典绑定后的句柄） |

#### IRI `parse` R1 规则

1. 非空  
2. 不含 ASCII 空白  
3. 必须包含 `:`（scheme 分隔）  

完整 RFC 3987 留待后续增强。

#### Literal 构造

- `Literal::new(value)`：按 `LiteralValue` 默认 XSD datatype  
- `Literal::string(...)`  
- `Literal::language_string(..., LanguageTag)`：datatype = `rdf:langString`  
- `Literal::typed(value, datatype)`  

`LiteralValue::lexical_form()` 提供稳定词法形式；`Decimal` 对非有限/科学计数场景回退为 `bits:<u64>` 以保持确定性。

### 3.3 知识对象容器（`domain::knowledge`）

> Statement（Triple/Quad）**不在本层**，由 `ontolith-rdf` 持有值类型，通过 `NodeId` / `Iri` 引用本层身份。

| 类型 | 说明 |
|------|------|
| `ObjectMetadata` | 键值标签袋；canonical 时按 key/value 排序 |
| `GraphId` | `Default` 或 `Named(Iri)` |
| `GraphStatistics` | triple_count / distinct_subjects / predicates / objects |
| `GraphObject` | Graph KO：header + graph_id + metadata + statistics |
| `DatasetObject` | Dataset KO：header + default_graph + named_graphs + metadata |
| `OntologyObject` | 特化 Dataset；可挂 tbox/abox/annotation/rule/provenance 图引用 |
| `RuleObject` | 规则 KO 占位（供 reasoner） |
| `VersionObject` | 版本谱系记录：target_id + target_version + parent_version |

#### Dataset 行为

- `DatasetObject::new`：创建对象，并自动创建 default `GraphObject`  
- `add_named_graph`：拒绝 default graph id；拒绝重复 named graph  
- `named_graph(&Iri)` / `graph_count()`

### 3.4 字典节点 ID（`domain::NodeId`）

| 项 | 说明 |
|----|------|
| 定义 | `NodeId(pub u64)` |
| 语义 | 字典层分配的 **库内不可变** 内部标识（SAS-0401 §5） |
| API | `new` / `get` / `Display`（`node:{id}`） / `CanonicalEncode` |
| 与 ObjectId 区别 | `ObjectId` = 逻辑/跨后端稳定 ID；`NodeId` = 当前库 epoch 内编码 ID |

### 3.5 规范编码（`domain::canonical`）

| 类型/Trait | 职责 |
|------------|------|
| `CanonicalWriter` | 追加 tag、u8、u64 LE、length-prefixed bytes/str |
| `CanonicalEncode` | `write_canonical` / `canonical_bytes` / `canonical_hex` |

编码约定（R1 基线）：

1. 变体以短 ASCII tag 开头（如 `I` IRI、`B` blank、`L` literal、`N` node、`GD`/`GN` graph id）  
2. 字符串：`u32 LE length || UTF-8 bytes`  
3. `u64`：小端 8 字节  
4. 映射类结构在编码前排序（见 `ObjectMetadata`）  
5. **不** 引入 HashMap 迭代顺序  

已实现 `CanonicalEncode` 的类型包括：`Iri`、`BlankNodeId`、`LiteralValue`、`Literal`、`Resource`、`NodeId`、`GraphId`、`ObjectMetadata`，以及 `str` / `String` / `u64`。

### 3.6 错误模型（`error::OntolithError`）

| 变体 | code | 用途 |
|------|------|------|
| `InvalidArgument` | `invalid_argument` | 入参校验失败 |
| `InvalidState` | `invalid_state` | 状态不允许该操作 |
| `NotFound` | `not_found` | 实体不存在 |
| `AlreadyExists` | `already_exists` | 唯一性冲突 |
| `Unsupported` | `unsupported` | 未实现/未启用能力 |
| `Storage` | `storage` | 抽象边界上的存储类失败 |

特性：

- 消息为 `&'static str`（低层廉价、无分配错误链）  
- 实现 `Display` + `std::error::Error`  
- `code()` / `message()` 稳定可观测  

兼容：既有 `InvalidState` / `Unsupported` 用法保持可用。

### 3.7 时间

- `TimestampMs = u64`：UTC 毫秒时间戳  
- **本层不读取系统时钟**；由调用方注入，保证可测性  

### 3.8 Crate 元信息

| 常量/函数 | 值 |
|-----------|-----|
| `CRATE_ID` | `"ontolith-core"` |
| `LAYER` | `"L0-core-knowledge-object"` |
| `healthcheck()` | 恒 `true`（骨架健康探针） |

---

## 4. 公共 API 导出面

通过 `ontolith_core::domain` 导出（保持下游 `use ontolith_core::domain::{NodeId, Iri, ...}` 兼容）：

```text
NodeId, TimestampMs
ObjectId, ObjectVersion, VersionId, ObjectType, ObjectState, KnowledgeObjectHeader
Iri, BlankNodeId, LanguageTag, LiteralValue, Literal, Resource, BoundResource
GraphId, GraphObject, GraphStatistics, DatasetObject, OntologyObject
ObjectMetadata, RuleObject, VersionObject
CanonicalEncode, CanonicalWriter
```

错误：`ontolith_core::error::OntolithError`。

---

## 5. 不变量与契约

1. **无环依赖**：core 不依赖任何 ontolith 上层 crate。  
2. **身份稳定**：`ObjectId` / 已分配 `NodeId` 在对象生命周期内不原地改写语义。  
3. **Header 完整性**：凡 KO 容器必须带 `KnowledgeObjectHeader`，且 `object_type` 与容器一致（Ontology 构造时会把 header 类型设为 `Ontology`）。  
4. **生命周期**：仅允许 §3.1 表中的迁移；逻辑删除优先于物理删除。  
5. **Canonical 确定性**：同一逻辑值 → 相同字节序列；与插入顺序无关的集合必须排序后编码。  
6. **错误可预测**：校验失败返回 `InvalidArgument`；状态错误返回 `InvalidState`；不使用 panic 表达业务失败（测试断言除外）。  
7. **向后兼容**：既有字段式构造（`Iri::new`、`NodeId::new`、`LiteralValue::*`）不得破坏 storage/query 编译。  

---

## 6. 测试与验收

### 6.1 单元测试（`domain::tests`）

| 用例 | 断言 |
|------|------|
| `object_id_rejects_empty` | 空 ID 拒绝 |
| `lifecycle_allows_normative_path_and_delete` | 主路径 + 删除 |
| `lifecycle_rejects_skip_ahead` | 禁止跳跃 |
| `iri_parse_baseline` | IRI 校验 |
| `resource_canonical_encoding_is_stable` | 资源编码稳定且区分类型 |
| `language_tag_normalizes_case` | 语言标签小写化 |
| `dataset_manages_named_graphs` | 命名图增查与重复拒绝 |
| `ontology_is_specialized_dataset` | Ontology 类型标记 |
| `metadata_canonical_ignores_insertion_order` | 元数据顺序无关 |
| `node_id_display_and_canonical` | NodeId 展示与编码 |
| `error_display_includes_code` | 错误码展示 |

### 6.2 回归

实现本层后，以下下游测试应保持绿色（不改其源码）：

- `ontolith-storage`  
- `ontolith-transaction`  
- `ontolith-query`  
- `ontolith-observability`  

### 6.3 验收标准（L0 Done）

- [x] SAS-0401 基础类别在类型系统中可表达（Resource/Graph/Dataset/Ontology/Rule/Version/Metadata；Statement 接口留给 L1）  
- [x] Header + 生命周期可执行  
- [x] Canonical 编码基线可用  
- [x] 错误枚举覆盖常见领域失败  
- [x] 无第三方依赖  
- [x] 单测通过且不破坏下游  

---

## 7. 与上层的边界

| 上层 | 应从 L0 取用 | 不应在 L0 实现 |
|------|----------------|----------------|
| `ontolith-rdf` | `NodeId`/`Iri`/`LiteralValue`/`Resource`/`GraphId`/`CanonicalEncode`/`OntolithError` | Triple 存储 |
| `ontolith-storage` | `NodeId`/`Iri`/错误类型 | KO 业务生命周期策略 UI |
| `ontolith-parser` | Dataset 相关身份类型（经 rdf） | 词法/语法 |
| `ontolith-reasoner` | `OntologyObject`/`RuleObject` | OWL 规则执行 |
| `ontolith-security` | 可引用 `ObjectId` 做审计主语 | 鉴权协议 |

---

## 8. 已知限制与后续

| 项 | 现状 | 后续层/工作 |
|----|------|-------------|
| Statement KO | 仅有 `ObjectType::Statement` | L1 挂 Triple/Quad + 可选 Header |
| IRI 校验 | 基线启发式 | 完整 IRI/URI 规范可选 feature |
| Literal Decimal | `f64` 位型确定性 | 十进制任意精度类型 |
| Canonical 规范文档 | 代码即规范 | 独立 RFC/编码规范文档（P1-04） |
| 序列化 Part II | 未做 | SAS-0401 Part II Metadata/Serialization |
| 应用服务 | application 占位 | 若需要 KO 仓储端口再扩 |

---

## 9. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | 首版：L0 实现同步功能说明 |

---

## 10. 相关代码索引

| 主题 | 路径 |
|------|------|
| Crate 入口 | `crates/ontolith-core/src/lib.rs` |
| 身份与生命周期 | `crates/ontolith-core/src/domain/identity.rs` |
| 资源 | `crates/ontolith-core/src/domain/resource.rs` |
| Graph/Dataset/Ontology | `crates/ontolith-core/src/domain/knowledge.rs` |
| Canonical | `crates/ontolith-core/src/domain/canonical.rs` |
| 错误 | `crates/ontolith-core/src/error.rs` |
| 单测 | `crates/ontolith-core/src/domain/mod.rs` (`tests` 模块) |
