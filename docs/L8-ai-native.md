# L8 — AI-Native 语义扩展立项

文档 ID: AI-L8-0001  
版本: 0.1.4  
状态: Active（R4 立项完成；P8-01 M1 语义核心 + M2 server 接线 + M3 持久化与增量更新 + P8-02 检索 KPI 门禁 + P8-03 代理集成扩展点完成）  
日期: 2026-08-09  
对应代码: `crates/ontolith-ai` + `crates/ontolith-storage`（`semantic` CF）  
计划: [Ontolith_Development_Plan.zh-CN.md](./Ontolith_Development_Plan.zh-CN.md) §6 R4 / Phase 8

---

## 1. 立项背景与目标

R1–R3 已完成或达成（R1 全表勾选、R2 门禁全绿、R3 范围要素多数已随 L5/L7 落地：
高可用故障转移演练 P7-01/04、强制租户隔离 P5-03、审计加固与 OIDC R2+）。按
PROGRESS §8 自底向上队列，光标移至 **L8 AI-Native 语义运行时扩展**（R4）。

Phase 8 目标（PLAN-0001）：

- 实现语义与向量桥接能力（P8-01）。
- 实现检索增强接口（P8-02）。
- 实现代理集成扩展点（P8-03）。

R4 退出标准：

- 扩展安全与兼容门禁通过（新增能力不破坏既有 SPARQL/RDF 语义、无未登记依赖、
  embedding 提供者可插拔、向量数据不污染主存储）。
- 检索与语义集成 KPI 达标（top-k 相关项命中、检索延迟预算、确定性可复现）。

## 2. 范围与非目标

| ID | 范围 | 非目标（本期） |
|----|------|----------------|
| P8-01 | 语义-向量桥接：RDF 项（IRI/字面量/词法）↔ 定长向量；可插拔 embedding 提供者；树内确定性 fallback；相似度计算；向量索引（内存首版） | 不引入外部 embedding 服务/SDK（无新 Tier A 依赖）；不做 ANN 近似索引 |
| P8-02 | 检索增强接口：`/semantic/search?q=&k=` 语义检索 API；top-k 相关项返回；与 SPARQL 结果 JSON 同构 | 不做 RAG 完整链路/文档切分 |
| P8-03 | 代理集成扩展点：`plugin-api` 增 `Retrieval` 能力；代理工具（tool）抽象：语义检索 → 语句/SHACL 验证 | 不做 MCP/外部协议绑定 |

## 3. 架构决策

### 3.1 可插拔 EmbeddingProvider（P8-01 核心抽象）

```text
EmbeddingProvider (trait)
 ├─ FeatureHashEmbedding   ← 树内确定性 fallback（默认，无外部依赖）
 └─ (后续) RemoteProvider   ← 外部服务适配（API key、缓存、超时；走 RFC 引入）
```

- `Embedding { dim: usize, values: Vec<f32> }`，L2 归一化后存储，相似度用余弦。
- `embed_text(&str) -> Result<Embedding>`：任意文本（查询串、词法形式）。
- `embed_term(&Term) -> Result<Embedding>`：IRI/字面量/语言标签按确定性规则投影。
- **确定性要求**：同输入同输出（跨进程、跨重启），满足「可复现」KPI 与 W3C 结果集
  比对风格；feature-hash 使用稳定哈希（FNV-1a 64 位，树内实现），不依赖随机化。

### 3.2 语义索引与检索（P8-02 接口基础）

- `SemanticIndex`：`term -> Embedding` 线性存储。M1 内存首版；M3 落盘为
  `ontolith-storage` 独立 `semantic` 列族（仿 L4 `raft` CF 的字节级原语
  `semantic_cf_*`），`crates/ontolith-ai` 内 `RocksSemanticIndex` 经该原语读写，
  向量数据与 RDF 数据面物理隔离（R4 门禁 4）。
- `SemanticSearchService`：查询文本 → embedding → top-k 余弦相似度 → 返回
  `(Term, score)`；k 有上限护栏（默认 10，硬上限 100）；索引容量上限
  `AUTO_INDEX_CAP` 在服务内统一执行（启动自动索引 / 显式 POST / 写回流共用）。
- 空索引 / 维度不匹配 / 非有限值（NaN/Inf）显式报错，不静默吞掉。

### 3.4 增量更新语义（M3，删改回流）

- 写提交后回流：ingest（`POST /data/*`）携带精确 `WriteOperation` 列表 → 精确
  术语差异（Put 项入索引，Delete 项在“全位置引用检查”后驱逐）；SPARQL Update
  未向网关暴露操作列表 → 全量存储差异对账（扫描存储术语集合与索引术语集合，
  新增入索引、消失项驱逐，仅写变化项）。
- 引用检查是位置无关的：一个 IRI 只要仍出现在任何三元组/四元组的
  主语/谓词/宾语任一位置即保留索引；仅当全无引用才驱逐（多三元组共享术语安全）。
- 删除与插入同一批次时按提交后状态判定，天然支持“删除+重建”更新形态。
- 索引容量上限同样约束回流（超出部分跳过，与启动自动索引一致）。

### 3.3 扩展安全与兼容门禁（R4 判据）

1. embedding 纯函数、无 I/O 副作用，不读写主存储；向量索引与 RDF 数据面隔离。
2. 依赖登记：本期零新增外部依赖（`std` only），`DEPENDENCY_REGISTER.md` 不变；
   未来引入外部 provider 必须先走 RFC/ADR。
3. 检索接口鉴权：复用 L5 `HeaderAuthenticator` 模式（Bearer/JWT），越权 403。
4. 兼容门禁：workspace 全量测试 + W3C 492/492 + SHACL 98/98 零漂移；clippy 零警告。

## 4. 里程碑（M1 起，随波次推进）

| 里程碑 | 内容 | 证据 |
|--------|------|------|
| M1（完成 2026-08-09） | `ontolith-ai` crate：EmbeddingProvider 抽象 + FeatureHashEmbedding + 余弦相似度 + 内存语义索引 + top-k 检索 + 测试 | crate 8 测全绿 |
| M2（完成 2026-08-09） | server 接线：`/semantic/search` + `/semantic/index` HTTP + 启动自动索引 + 鉴权/审计复用 + `/health`·`/admin/config` 姿态 | server 54 测（+5 语义） |
| M3（完成 2026-08-09） | 持久化语义索引（RocksDB 独立 `semantic` CF + `RocksSemanticIndex`）+ 增量更新语义（删改回流：ingest 精确差异 + SPARQL Update 存储差异对账 + 位置无关引用检查）；另修复 `InMemoryDictionary::contains_value` 变更副作用（非破坏性成员探测） | storage 52→53 测（semantic CF 重启往返）、ai 8→13 测（Rocks 索引重启持久/批删）、server 54→57 测（ingest 回流 + SPARQL DELETE 驱逐 + 共享术语保留 + RocksDB 重启持久） |
| P8-02（完成 2026-08-09） | 检索 KPI 门禁：扁平行主序矩阵重构 + const generic `dot_const::<256>` 向量化热路径 + `select_nth_unstable_by` 部分选择；`ontolith-compliance/p802_retrieval_gate`（确定性/相关命中/延迟预算）+ CI `retrieval-gates` 作业 + 语义 bench 阈值/趋势 | 10k 语料 `search_embedding` 实测 0.33–0.52ms < 1ms（原 1.57–2.26ms）；gate 3 测全绿（release profile） |
| M4（完成 2026-08-09） | P8-03 代理集成扩展点：plugin-api `Retrieval` 能力 + `AgentTool` 契约（`ToolDefinition`/`ToolInput`/`ToolOutput`/`RetrievalResult`）+ `ontolith-ai` `SemanticRetrievalTool` 示例工具（语义检索 → 可读 term/kind/score 输出，可继续链到语句/SHACL 验证工具） | plugin-api 0→4 测、ai 13→16 测；workspace + W3C + SHACL 零漂移 |
| R4 | 检索 KPI 与扩展安全/兼容门禁全绿 | ACC-R4 验收包 |

## 7. 环境契约与 API（P8-01 M2/M3）

| 环境变量 | 默认 | 说明 |
|----------|------|------|
| `ONTOLITH_SEMANTIC_ENABLED` | 关 | `1/true/on` 开启语义检索（默认关闭，保持网关行为不变） |
| `ONTOLITH_SEMANTIC_DIM` | 256 | 特征哈希 embedding 维度 |
| `ONTOLITH_SEMANTIC_AUTO_INDEX_CAP` | 100000 | 启动时自动索引的存储项上限（主语+谓词+宾语，bnode 主语跳过） |

API（鉴权/审计复用 L5 模式，`semantic:read` / `semantic:write`）：

- `GET /semantic/search?q=<text>&k=<n>` → `{"dim":256,"indexed":N,"query":...,"hits":[{"term":{"type":"uri|literal|bnode","value":...},"score":0.xxxxxx}]}`；`k∈[1,100]` 自动截断；未启用返回 501。
- `POST /semantic/index?term=<iri>` 或 `?terms=<iri1>,<iri2>`（URL 编码）→ `{"indexed":n,"total":m}`；幂等去重。
- `/health` 与 `/admin/config` 暴露 `semantic: on|off` 姿态。

持久化契约（M3，RocksDB 后端）：术语→向量存储于独立 `semantic` 列族，键为
RFC-0001 规范编码（`encode_term`），值为 `u32 BE 维度 ‖ f32 LE 向量`；写走引擎
耐久路径（默认 fsync WAL），重启后索引完整恢复（含非存储术语，如显式
`POST /semantic/index` 的条目）。内存后端保持原有行为。

已解决限制（M2→M3）：删除/更新已回流索引——ingest 与 SPARQL Update 提交后，
新术语入索引、无引用术语被驱逐；重启后索引持久不丢。

## 8. 代理工具契约（P8-03，M4）

- `PluginCapability::Retrieval`：插件能力枚举新增检索能力，`PluginManifest.capabilities` 可声明。
- `AgentTool` trait：`definition() -> &ToolDefinition`（名称/描述/参数 schema/能力声明）+ `call(&ToolInput) -> Result<ToolOutput, OntolithError>`；输入为字符串键值对（`get`/`get_required`），输出为文本或结构化 `RetrievalResult`（`query` + `hits[]`：可读 term 文本 + `uri|literal|bnode` kind + score）。
- 确定性：工具对相同输入必须字节级一致（R4 KPI 同源要求）；工具为无副作用纯函数。
- 示例工具：`ontolith-ai` `SemanticRetrievalTool`（`semantic_retrieval`，q 必填 / k 可选 [1,100]，空查询与非法 k 返回稳定错误）。
- 扩展点：`AgentTool` 契约不绑定 MCP/外部协议；后续「语义检索 → 语句/SHACL 验证」以同契约链式工具实现（检索命中经 SPARQL 取语句、经 SHACL 校验后返回 `ToolOutput::Text`）。

## 5. KPI（R4 检索与语义集成）

- 确定性：同查询两次检索结果字节级一致（含 score 排序稳定）。
- 相关命中：构造同义/相关语料，top-1 命中率 100%（受控语料门禁）。
- 延迟预算：内存索引 top-10 检索 < 1ms（10k 项语料；CI bench 观测）。
- 兼容：workspace 全量 + W3C 492/492 + SHACL 98/98 零漂移。

P8-02 实测（2026-08-09，本机 Intel Core Ultra 7 155H / release）：

| KPI | 预算 | 实测 | 状态 |
|-----|------|------|------|
| `search_embedding` top-10（10k 项 / 256 维） | < 1ms | **0.33–0.52ms**（优化前 1.57–2.26ms；裸扫描约 0.32ms，受 10MB 语料内存带宽约束） | ✅ 达标 |
| 非 256 维回退路径（128 维） | < 1ms | 0.77ms | ✅ 达标 |

实现要点：`InMemorySemanticIndex` 采用扁平行主序矩阵（`values: Vec<f32>`，行 = 256 个 f32）；
默认维度走 `dot_const::<256>`——`try_into` 借出 `&[f32; 256]` 定长数组，LLVM 全展开并 SSE
向量化（运行时边界下同一循环仅得标量，见 P8-02 提交说明）；top-k 用
`select_nth_unstable_by` 部分选择 + 前 k 排序，替代全量 `sort_by`。门禁：
`ontolith-compliance/tests/p802_retrieval_gate.rs`（CI `retrieval-gates` 作业，release profile）
+ `scripts/check-semantic-bench-thresholds.sh`（`semantic-bench` 阈值断言 + `benchmarks/trends/semantic-bench.jsonl` 趋势，仿 P7-02）。

## 6. 已识别风险与缓解

- 外部 embedding 服务不可用：默认树内 fallback 保底，远程 provider 失败降级并告警。
- 向量维度爆炸：维度固定（默认 256），feature-hash 碰撞可控；ANN 留待规模证据。
- 语义检索误用为“语义正确性”保证：文档明确检索是近似召回，验证仍走 SPARQL/SHACL。
