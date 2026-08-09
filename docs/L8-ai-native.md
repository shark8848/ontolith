# L8 — AI-Native 语义扩展立项

文档 ID: AI-L8-0001  
版本: 0.1.1  
状态: Active（R4 立项完成；P8-01 M1 语义核心 + M2 server 接线完成）  
日期: 2026-08-09  
对应代码: `crates/ontolith-ai`（新建）  
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

- `SemanticIndex`：`term -> Embedding` 线性存储（内存首版；RocksDB 持久化留待
  P8-02 里程碑，避免本期引入存储面耦合）。
- `SemanticSearchService`：查询文本 → embedding → top-k 余弦相似度 → 返回
  `(Term, score)`；k 有上限护栏（默认 10，硬上限 100）。
- 空索引 / 维度不匹配 / 非有限值（NaN/Inf）显式报错，不静默吞掉。

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
| M3 | 持久化语义索引（RocksDB 独立 CF）+ 增量更新语义 | storage 复用 + 重启持久测试 |
| M4 | P8-03 代理集成扩展点：plugin-api `Retrieval` 能力 + AgentTool 抽象 | plugin-api + 示例工具 |
| R4 | 检索 KPI 与扩展安全/兼容门禁全绿 | ACC-R4 验收包 |

## 7. 环境契约与 API（P8-01 M2）

| 环境变量 | 默认 | 说明 |
|----------|------|------|
| `ONTOLITH_SEMANTIC_ENABLED` | 关 | `1/true/on` 开启语义检索（默认关闭，保持网关行为不变） |
| `ONTOLITH_SEMANTIC_DIM` | 256 | 特征哈希 embedding 维度 |
| `ONTOLITH_SEMANTIC_AUTO_INDEX_CAP` | 100000 | 启动时自动索引的存储项上限（主语+谓词+宾语，bnode 主语跳过） |

API（鉴权/审计复用 L5 模式，`semantic:read` / `semantic:write`）：

- `GET /semantic/search?q=<text>&k=<n>` → `{"dim":256,"indexed":N,"query":...,"hits":[{"term":{"type":"uri|literal|bnode","value":...},"score":0.xxxxxx}]}`；`k∈[1,100]` 自动截断；未启用返回 501。
- `POST /semantic/index?term=<iri>` 或 `?terms=<iri1>,<iri2>`（URL 编码）→ `{"indexed":n,"total":m}`；幂等去重。
- `/health` 与 `/admin/config` 暴露 `semantic: on|off` 姿态。

已知限制（M2）：增量索引仅限显式 `POST /semantic/index` 与启动自动索引；删除/更新不回流索引（M3 持久化 + 增量更新语义解决）。

## 5. KPI（R4 检索与语义集成）

- 确定性：同查询两次检索结果字节级一致（含 score 排序稳定）。
- 相关命中：构造同义/相关语料，top-1 命中率 100%（受控语料门禁）。
- 延迟预算：内存索引 top-10 检索 < 1ms（10k 项语料；CI bench 观测）。
- 兼容：workspace 全量 + W3C 492/492 + SHACL 98/98 零漂移。

## 6. 已识别风险与缓解

- 外部 embedding 服务不可用：默认树内 fallback 保底，远程 provider 失败降级并告警。
- 向量维度爆炸：维度固定（默认 256），feature-hash 碰撞可控；ANN 留待规模证据。
- 语义检索误用为“语义正确性”保证：文档明确检索是近似召回，验证仍走 SPARQL/SHACL。
