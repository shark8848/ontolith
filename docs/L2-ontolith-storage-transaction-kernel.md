# L2 — Storage & Transaction Kernel 功能说明

文档 ID: IMPL-L2-0001  
版本: 3.0.0  
状态: Implemented (in-memory + RocksDB durable adapter)  
日期: 2026-07-17  
对应 crate:

- `crates/ontolith-storage`
- `crates/ontolith-transaction`

规范依据:

- [SAS-0001](./Ontolith_Software_Architecture_Specification.md) §6 / §8
- [SAS-0400](./Ontolith%20Software%20Architecture%20Specification%20%20Volume%2004.md)
- [L0](./L0-ontolith-core-Knowledge-Object-Foundation.md) · [L1](./L1-ontolith-rdf-Statement-Graph-Dataset.md) · [L3](./L3-ontolith-parser-query.md)

---

## 1. 相对架构的差距分析（v1 内存基线 → v2）

| SAS-0001 / 计划要求 | v1 基线 | v2 本增量 | 仍延期 |
|---------------------|---------|-----------|--------|
| 六置换索引 SPO/SOP/PSO/POS/OSP/OPS | 仅 SPO/POS/OSP，commit 全量 rebuild | **六置换 + 增量 insert/remove** | RocksDB 物理落盘 |
| 命名图（quad model） | 线性 `Vec<Quad>` 过滤 | **GraphIndex**（by_graph + 精确删） | GSPO 全置换 |
| 字典稳定 ID / epoch | encode/decode | + `epoch()` API | 持久字典 / GC |
| WAL 耐久后 ACK | 内存 WAL | 同左（契约不变） | 磁盘 fsync |
| 索引维护可配置 sync/async | 隐式 sync rebuild | `IndexMaintenance::Sync` 声明；增量 sync | Async 实现 |
| 精确删除 / 幂等写入 | 仅 subject 前缀删；Put 可重复 | **DeleteTriple/DeleteQuad**；**Put 集合语义去重** | 压缩/vacuum |
| 存储统计供优化器 | 无 | **`StorageStats`** | 直方图/代价模型 |
| 多绑定模式探测 | 单键 lookup | **`triples_matching_in_txn` / `matching_in_txn`** | 统计驱动选路 |
| 一致性级别 API | 无 | L0 `ConsistencyLevel` + `snapshot_with` | 集群会话/副本陈旧度 |
| 真 MVCC 读快照 | 仅 txn staged 叠加 | 同左（语义文档化） | 版本链 / 快照隔离 |
| RocksDB 适配 | 无 | **`RocksDbStorageEngine`（feature `rocksdb-backend`）** | 纯 CF 扫描索引（当前仍为内存二级索引） |

**结论：** v2 把「内存原型」推进为 **生产形状的单机内核**；**v3 落地 RocksDB 耐久适配**（进程崩溃可恢复字典与语句）。纯 CF 索引扫描与真 MVCC 仍属后续。

---

## 2. 层定位

```text
L3  query/parser  ← 经 matching() / DictionaryCodec 消费 L2
L2  storage + transaction   ← 本层
    ├── InMemoryStorageEngine
    └── RocksDbStorageEngine (feature rocksdb-backend)
L1  rdf
L0  core (ConsistencyLevel, Canonical, NodeId)
```

---

## 3. v2 功能清单

### 3.1 增量六置换索引（`infrastructure/indexes.rs`）

| 索引 | 键 | 用途 |
|------|----|------|
| SPO | subject | 主路径 / subject scan |
| SOP | subject | 同 subject 下 object 优先场景 |
| PSO | predicate | 谓词前缀变体 |
| POS | predicate | L3 谓词 lookup |
| OSP | object canonical | L3 object lookup |
| OPS | object canonical | object+predicate 变体 |

- **增量**：`insert` / `remove_exact` / `remove_by_subject`，commit 路径 **不再** `rebuild_indexes`  
- **去重**：`triple_key` HashSet；重复 `PutTriple` 为 no-op  
- **物理键**：`physical_index_keys` 返回 6 组确定性字节（供未来 LSM）

### 3.2 命名图索引

- `GraphIndex`：`by_graph` + 全量列表 + 精确删  
- `quads_by_graph_in_txn`：支持 staged PutQuad/DeleteQuad 可见性  

### 3.3 写操作扩展

```text
PutTriple | PutQuad | DeleteTriple | DeleteQuad | DeleteKey(subject prefix)
```

| 操作 | 语义 |
|------|------|
| Put* | 集合语义；已存在则忽略 |
| DeleteTriple/Quad | 精确删除；不存在则 no-op（幂等） |
| DeleteKey spo+subject | 删该 subject 全部 default triples + 匹配 quads |

### 3.4 统计 `StorageStats`

```text
triple_count, quad_count,
distinct_subjects/predicates/objects,
named_graph_count, dictionary_entries,
pending_transactions, wal_records, index_kinds_active(=6)
```

供 L3 优化器与 observability 采样。

### 3.5 快照与一致性

```rust
snapshot() -> Strong, read_txn=None
snapshot_with(ConsistencyLevel, Option<TxnId>)
```

L0 新增 `ConsistencyLevel { Strong, Session, Eventual }`。

### 3.6 字典

- `epoch()`：映射表世代（clear/replace 时递增；当前仅暴露，默认 0）  
- `len` / `contains_value`（查询不分配）

### 3.7 RocksDB 耐久适配（v3）

| 项 | 说明 |
|----|------|
| Feature | `rocksdb-backend`（workspace 默认开启） |
| 入口 | `open_durable_engine(path)` / `RocksDbStorageEngine::open` |
| 列族 | `meta`, `dict_fwd`, `dict_rev`, `triples`, `quads`, `wal` |
| 写路径 | stage → WAL CF；commit → triples/quads + WAL Committed（同一 RocksDB batch） |
| 读路径 | 打开时从 CF 重建内存六索引；查询走内存索引（与 InMemory 同 API） |
| 字典 | 双向 CF 持久化；`encode_node` 幂等 |
| 隔离 | `rocksdb::` 仅在 `infrastructure/rocks.rs` |
| 治理 | [ADR-0001](../adr/0001-rocksdb-storage-backend.md)、[依赖登记](./DEPENDENCY_REGISTER.md) |

### 3.8 事务内核（`ontolith-transaction`）

未改行为：begin/commit/abort/timeout/max_active/metrics。  
与存储协作顺序仍为：**storage commit → manager commit**。

### 3.9 跨层消费（L3）

- `QueryReadService::matching` → `TripleRepository::matching_in_txn`  
- BGP 执行用多绑定探测，减少先扫后滤  
- `QueryRequest.consistency` 声明读一致性（单机 Strong/Session 等价）  
- L3 **不** 依赖 RocksDB；通过 `Arc<dyn StorageEngine>` / repo 注入即可切换后端  

---

## 4. 模块结构（v3）

```text
ontolith-storage/src/
├── domain/
│   ├── mod.rs          WriteOperation(+Delete*), StorageStats, SnapshotRef(+consistency)
│   └── encoding.rs     IndexKind 六置换编码
├── application/mod.rs  StorageEngine(+stats/matching/snapshot_with), repos(+delete/matching)
└── infrastructure/
    ├── codec.rs        后端无关二进制编解码（triple/quad/WAL）
    ├── indexes.rs      TripleIndexes + GraphIndex（增量）
    ├── rocks.rs        RocksDbStorageEngine（feature 门控）
    └── mod.rs          InMemory* 引擎 / WAL / 仓储 / 测试
```

---

## 5. 不变量

1. 默认图语句 **集合语义**（无重复 triple）。  
2. 六索引与 `default_graph` 在每次成功 commit/delete 后一致。  
3. staged 仅对同 `TxnId` 可见。  
4. 厂商 API 不得上浮超过 infrastructure。  
5. `index_kinds_active == 6` 表示内存引擎已维护全置换。  

---

## 6. 测试

| 套件 | 数量 | 新增覆盖 |
|------|------|----------|
| ontolith-storage | **35** | 上表 + codec 往返 + RocksDB reopen/abort/delete 持久化 |
| ontolith-query | 21 | matching 路径回归绿 |
| ontolith-core | 12 | ConsistencyLevel |

---

## 7. 已知限制（v3 边界）

1. RocksDB 读路径仍依赖 **打开时重建的内存二级索引**（非纯 CF 前缀扫描）。  
2. **无真 MVCC 版本链** — 读 = 已提交 ∪ 本 txn staged。  
3. **IndexMaintenance::Async** 仅枚举预留。  
4. **命名图** 无六置换，仅 graph→quad。  
5. **字典 GC / 压缩 / vacuum** 未做。  
6. 构建需要本机能编译 `librocksdb-sys`（C++ 工具链）。  
7. SOP/PSO/OPS 已维护，L3 matching 组合过滤；未单独暴露 SOP 扫描 API。  

---

## 8. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | 内存基线：SPO/POS/OSP + WAL + 事务文档 |
| 2026-07-17 | 2.0.0 | 增量六索引、精确删/去重、GraphIndex、Stats、ConsistencyLevel、matching；L3 接入 |
| 2026-07-17 | 3.0.0 | RocksDB 适配（CF 布局、崩溃恢复、feature 门控、ADR-0001） |

---

## 9. 代码索引

| 主题 | 路径 |
|------|------|
| 增量索引 | `crates/ontolith-storage/src/infrastructure/indexes.rs` |
| 编解码 | `crates/ontolith-storage/src/infrastructure/codec.rs` |
| RocksDB 适配 | `crates/ontolith-storage/src/infrastructure/rocks.rs` |
| 内存引擎 | `crates/ontolith-storage/src/infrastructure/mod.rs` |
| 契约 | `crates/ontolith-storage/src/application/mod.rs` |
| 一致性 | `crates/ontolith-core/src/domain/consistency.rs` |
| 查询 matching | `crates/ontolith-query/src/infrastructure/execute.rs` |
| ADR | `adr/0001-rocksdb-storage-backend.md` |
| 依赖登记 | `docs/DEPENDENCY_REGISTER.md` |
