# L2 存储契约：并发字典与接口版本冻结

- 状态: Active
- 日期: 2026-08-07
- 范围: P1-02（并发字典契约）、P1-03（存储抽象接口版本冻结）
- 关联: [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)、[ADR-0001](../adr/0001-rocksdb-storage-backend.md)、[L2 事务内核文档](./L2-ontolith-storage-transaction-kernel.md)

---

## Part A — 并发字典契约（P1-02）

### 适用范围

`crates/ontolith-storage/src/application/mod.rs::DictionaryCodec` 及其全部实现：
- `InMemoryDictionary`（`infrastructure/mod.rs`）
- `RocksDbStorageEngine`（`infrastructure/rocks.rs`）

### 语义保证

| 保证 | 说明 | 违反后果 |
|------|------|----------|
| 线程安全 | 所有实现 `Send + Sync`；内存侧 `RwLock` 保护双映射，Rocks 侧由后端 CF 原子批写保护 | 数据竞争/悬垂引用 |
| 分配单调 | `NodeId` 1-based 单调递增；`encode_node` 对已存在值幂等返回既有 id | 索引键冲突 |
| 不可变性 | `NodeId` 在字典 epoch 内恒定（SAS-0401 §5）；`decode_node` 返回克隆字符串，不暴露内部借用 | 调用方持有失效 id |
| 原子性 | RocksDB 侧 fwd/rev/meta 三写同一 batch；`next_node_id` 崩溃后从 `meta` 恢复 | 半写字典/重复分配 |
| 确定性 | 同 lexical form → 同 id（进程内）；epoch 递增即旧映射失效 | 跨 epoch 引用错乱 |

### 并发行为细则

1. `encode_node` 与 `decode_node` 可任意线程并发调用；`encode_node` 的分配临界区在内存侧由写锁串行化。
2. `encode_node` 不要求调用方预取锁；实现内部加锁，禁止嵌套调用同一字典（死锁风险）。
3. `decode_node(Nonexistent) -> None`；`encode_node` 永不返回 `None`。
4. `contains_value`/`contains_node` 语义基于 `encode`+`decode` 往返，允许实现按需优化。
5. 上层（query/reasoner）只经 `DictionaryCodec` trait 访问字典，禁止依赖具体基础设施类型。

### 测试基线

- storage crate 字典单测（编码幂等、解码往返、并发读写、RocksDB reopen 恢复）。
- reasoner/query 依赖字典的集成测试均以 `InMemoryDictionary` 为基座，契约变化须同步验证。

---

## Part B — 存储抽象接口版本冻结（P1-03）

### 冻结基线

接口版本 **0.1.0**（2026-08-07 定稿），冻结以下应用层契约：

| 契约 | 关键方法/语义 |
|------|----------------|
| `DictionaryCodec` | `encode_node`/`decode_node`/`len`/`epoch`（见 Part A） |
| `WriteAheadLog` | `append`/`entries`/`truncate_prefix`（WAL 记录 Staged/Committed/Aborted） |
| `StorageEngine` | 写生命周期（`apply_write_batch` → `commit_transaction`/`abort_transaction`）；`snapshot_with(consistency, txn)`；`delete_by_key`；`stats`；`index_maintenance`；六索引查询族（`triples_by_*_in_txn`/`triples_matching_in_txn`/`quads_by_graph_in_txn`/`quads_matching_in_graph`） |
| `TripleRepository` | `insert`/`delete`/`all_in_txn`/`by_*_in_txn`/`matching_in_txn`（默认实现基于 StorageEngine 过滤） |

### 冻结规则

1. **上层依赖约束**：query/server/reasoner 只依赖 application 层 trait；基础设施类型（`InMemoryDictionary`、`RocksDbStorageEngine`）仅在测试与工厂中使用。
2. **集合语义**：`PutTriple`/`PutQuad` 为集合语义（重复插入为 no-op）；删除幂等。
3. **索引维护**：`IndexMaintenance::Sync` 为默认且正确性优先；`Async` 为保留位，未启用。
4. **物理键**：六置换索引键字节格式由 [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md) 定稿，后端无关。
5. **变更流程**：任何破坏性变更（签名、语义、物理格式）须先经 RFC/ADR 评审并在本文件登记版本升级；非破坏性扩展（新增默认方法）不受限。

### 版本变更登记

| 版本 | 日期 | 变更 | 依据 |
|------|------|------|------|
| 0.1.0 | 2026-08-07 | 首版冻结（本文档） | P1-03 |
| — | — | （后续破坏性变更在此登记） | — |

---

## 关联更新

- P0-04：首个实质 RFC（[RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)）落地。
- P1-04：确定性标识与规范化编码规则 + 磁盘布局由 RFC-0001 固化。
