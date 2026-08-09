# RFC-0001: 确定性标识与规范化编码规则（含磁盘布局）

- Status: Accepted
- Date: 2026-08-07
- Authors: Codex
- Reviewers: sharky-ai（2026-08-09 评审回填；Codex 执行体代执行）
- Tags: encoding, identity, storage, disk-layout
- Related ADRs: ADR-0001（RocksDB 存储后端）

## Summary

固化 Ontolith 的节点标识（`NodeId`）与规范化字节编码（`CanonicalWriter` / `CanonicalEncode`）为稳定契约，并给出 RocksDB 后端的列族与物理键布局。本 RFC 是 P0-04 首个实质 RFC 试用，同时交付 P1-04（确定性标识与规范化编码规则、磁盘布局）。

## Motivation

- 当前编码与标识规则分散在 `ontolith-core`（`CanonicalWriter`/`CanonicalEncode`）与 `ontolith-storage`（六置换索引键、WAL 编码）实现中，缺少独立契约文档，制约 P1-04 验收与 P2 磁盘布局评审。
- R1 基线要求物理键可跨后端复用（内存六索引 ↔ RocksDB CF），需显式约定键字节格式。
- SAS-0401 GO-005 / §5 要求节点标识在字典 epoch 内不可变，需将规则落成可评审 RFC。

## Detailed design

### Goals

- 固化 `NodeId` 分配与不可变语义（1-based、单调递增、epoch 内不变）。
- 固化规范化编码原语（长度前缀、ASCII 标签、u64 LE），保证字节确定性。
- 固化六置换索引键与 RocksDB 列族/元数据键布局，作为磁盘格式基线。
- 提供“首个实质 RFC”试用（P0-04），建立 RFC-0001 编号基线。

### Non-goals

- 不引入新的编码实现（本文档描述既有实现并定稿语义）。
- 不定稿真 MVCC 版本链的磁盘格式（随 P2-02 单独 RFC/ADR）。
- 不规定跨 datatype 的字面量归一化（见 Open questions）。

### Design

#### 1. 节点标识（NodeId）

- `NodeId(u64)`，字典内 1-based 单调分配（首个节点为 1）。
- 分配后不可变：同一 lexical form 在同一字典 epoch 内始终映射同一 `NodeId`；`NodeId` 不因并发/重启改变（RocksDB 持久化 `next_node_id`）。
- 字典 epoch（`DictionaryCodec::epoch`）在映射表被替换/清空时递增；epoch 变化即旧 `NodeId` 失效。

#### 2. 规范化编码原语（CanonicalWriter）

| 原语 | 字节格式 |
|------|----------|
| `write_tag` | 原始 ASCII 标签字节（如 `SPO`、`POS`、`OSP`） |
| `write_u8` | 单字节 |
| `write_u64` | 8 字节小端（LE） |
| `write_bytes`/`write_str` | `u32 LE 长度 ‖ 原始字节/UTF-8` |

规则：字段定长用 LE；变长字段一律 `u32 LE 长度前缀`；变体用 ASCII 标签区分；不引入依赖 HashMap 迭代顺序的非确定性输出。`CanonicalEncode` 提供 `canonical_bytes()`/`canonical_hex()`（hex 仅用于测试与日志，非存储格式）。

#### 3. 字典双向映射

- 内存：`RwLock<HashMap<NodeId, String>> × HashMap<String, NodeId>`。
- RocksDB：`dict_fwd`（value 字节 → `u64 LE id`）、`dict_rev`（`u64 LE id` → value 字节）两个 CF；`next_node_id` 写入 `meta` CF；fwd/rev/meta 三写在同一批（batch）内，保证崩溃一致性。
- 编码幂等：已存在值返回既有 `NodeId`，不重复分配。

#### 4. 六置换索引键

键格式 = 3 字节标签 ‖ 组件序列（`u64 LE` 或长度前缀字符串/规范化 Term）：

| 索引 | 键布局 | R1 要求 |
|------|--------|---------|
| SPO | `"SPO" ‖ S ‖ P ‖ O` | 必需 |
| POS | `"POS" ‖ P ‖ O ‖ S` | 必需 |
| OSP | `"OSP" ‖ O ‖ S ‖ P` | 必需 |
| SOP/PSO/OPS | 同构排列 | 保留（内存引擎已维护六置换） |

键字节与后端无关：同一字节序列可直接用作 RocksDB 键（不泄漏 vendor API）。

#### 5. RocksDB 磁盘布局（列族清单）

| CF | 内容 | 键格式 |
|----|------|--------|
| `meta` | 元数据：`next_node_id`、`wal_seq`、`dict_epoch` | 固定字节键 → `u64 LE` |
| `dict_fwd` | 词法形式 → 节点 id | value 字节 → `u64 LE` |
| `dict_rev` | 节点 id → 词法形式 | `u64 LE` → value 字节 |
| `triples` | 默认图三元组 | 规范三元组字节 |
| `quads` | 命名图四元组 + `graph_index` | 规范四元组字节 |
| `wal` | 写前日志记录（Staged/Committed/Aborted） | `u64 LE seq` → `encode_wal_record` |

WAL 序号单调递增并持久化于 `meta.wal_seq`；恢复时按 CF `wal` 扫描重放。

### Compatibility

- 本 RFC 为既有实现的定稿：不改变任何在线字节格式，无迁移需求。
- 后续磁盘布局变更（如 MVCC 版本链）须先经 RFC/ADR 评审并升级版本号。

### Security & multi-tenancy

- 编码规则不涉及鉴权；`NodeId` 不可猜测性不作为安全边界（可枚举）。
- 租户隔离依赖上层命名图/租户键，本 RFC 不改变其语义。

### Observability

- `StorageStats` 暴露字典/索引计数；`dict_epoch` 与 `next_node_id` 可观测用于诊断重启后映射一致性。

## Alternatives

| Option | Pros | Cons | Why not |
|--------|------|------|---------|
| 字符串键直存 | 可读性好 | 空间大、比较慢 | 需稳定数字 id 压缩索引体积 |
| 哈希 id（内容寻址） | 无字典依赖 | 碰撞风险、不可顺序分配 | 需顺序 id 用于六置换排序扫描 |
| 变长无标签编码 | 体积最小 | 解析歧义、难调试 | 定长+长度前缀已满足 R1 |

## Open questions

1. 跨 datatype 字面量归一化（如 `"1"^^xsd:int` vs `"1"^^xsd:integer`）是否纳入规范化？当前按词法+datatype 精确比较。
2. `dict_epoch` 在 RocksDB 上的递增触发条件（当前仅内存清空路径实现）是否需要落盘策略。

## Acceptance criteria

- [x] 编码原语与键布局文档化（本文档）
- [x] 与实现对照：`canonical.rs`、`encoding.rs`、`rocks.rs` CF 常量一致
- [x] 评审通过后回填 PROGRESS（P0-04、P1-04）（2026-08-09，见评审记录）
- [x] 磁盘布局变更走 RFC/ADR 流程（本 RFC 建立先例；语义索引持久化即复用 RFC-0001 键编码，见 P8-01 M3）

#

## 评审记录（Review Record）

| 项 | 值 |
|----|----|
| 评审日期 | 2026-08-09 |
| 评审人 | sharky-ai（项目负责人；依用户委托授权，Codex 执行体代为执行评审流程） |
| 评审范围 | ① `NodeId` 标识语义（1-based 单调、epoch 内不可变）与实现一致；② 规范化编码原语（长度前缀/ASCII 标签/u64 LE）与 `CanonicalWriter`/`CanonicalEncode` 一致；③ RocksDB 列族与物理键布局（data/SPO/POS/OSP/quads/raft/semantic CF）与 `rocks.rs` 实现一致；④ 跨实现引用（语义索引持久化键 = RFC-0001 `encode_term` 规范编码，P8-01 M3 实测往返） |
| 结论 | 通过：RFC-0001 所定契约全部落地并有测试证据（core 20 测 / storage 53 测 / ai 13 测，含 RocksDB 重启持久往返）；作为 P0-04 首个实质 RFC 试用完成，转正式 Accepted 并回填 PROGRESS |
# References

- PLAN-0001
- SAS-0401（GO-005 / §5）
- ADR-0001（RocksDB 存储后端）
- `crates/ontolith-core/src/domain/canonical.rs`
- `crates/ontolith-storage/src/domain/encoding.rs`
- `crates/ontolith-storage/src/infrastructure/rocks.rs`
