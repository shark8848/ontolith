# ADR-0006: Remote embedding provider + ANN index (L8 P8-01)

- Status: Accepted
- Date: 2026-09-02
- Deciders: sharky-ai（项目负责人；Codex 执行体代为执行决策流程）
- Tags: l8, ai, embedding, ann, tier-b

## Context

L8（[L8-ai-native.md](../docs/L8-ai-native.md)）P8-01 交付了可插拔
`EmbeddingProvider` + 树内确定性 `FeatureHashEmbedding` 回退 + 语义索引，
并明确将「外部 embedding 服务/SDK」与 ANN 近似索引列为后续轨（非目标，
走 RFC 引入）。本期把这两个后续轨落地，约束不变：

- 树内确定性 fallback 必须保留（无 `ONTOLITH_SEMANTIC_EMBEDDING_URL`
  时行为与 0.1.70 完全一致；feature-hash 仍为默认）。
- 外部服务适配不引入新的 Tier A 依赖：`ureq` 已在 lock（`log-center-sdk`
  传递依赖），按 Tier B 登记。
- 跨进程、跨版本确定性（R1–R2 既有门禁文化）：LSH 投影矩阵用固定种子
  SplitMix64 生成，无外部 RNG。
- 检索是近似召回，验证仍走 SPARQL/SHACL（L8 §3.2 既有语义）。

## Decision

1. **`RemoteHttpEmbeddingProvider`**（`crates/ontolith-ai`，
   `infrastructure/remote.rs`，ADR-0006）：
   - 外部 HTTP/JSON embedding 服务，OpenAI 兼容响应形
     `{"data":[{"embedding":[...]}]}`；请求体 `{"model","input"}`。
   - 可选 bearer API key（`Authorization: Bearer <key>`）；每请求超时
     （`ureq::Agent` timeout）；按输入文本的 FIFO 上限缓存（默认 4096 条）。
   - 维度不匹配（响应维度 ≠ 配置 `dim`）确定性报错；返回向量 L2 归一化
     与树内提供者一致（余弦退化为点积）。
   - 传输层为可注入 trait `RemoteHttpTransport`（生产 `UreqHttpTransport`，
     测试用内存 fake，零 socket）。
   - `embed_term` 复用共享确定性投影 `term_text`（与
     `FeatureHashEmbedding` 同一映射，RDF 项跨提供者稳定）。
2. **`LshSemanticIndex`**（`infrastructure/approx.rs`）：确定性多探针
   超平面 LSH 近似索引。
   - 投影矩阵 `bits × dim` 由固定种子 SplitMix64 生成；桶键为符号位。
   - 查询探针：查询桶 + Hamming 半径 `probes`（默认 2）内全部桶，仅对候选
     做精确点积 top-k；候选不足 `k` 或条目数 ≤ `exact_below` 时回退全量
     扫描 —— 结果永不截断，只可能重排。
   - 同一 `SemanticIndex` trait；`SemanticSearchService::with_lsh` 暴露。
3. **Server 接线**（`ontolith-server`）：`SemanticConfig` 增
   `ONTOLITH_SEMANTIC_EMBEDDING_URL/API_KEY/MODEL/TIMEOUT_SECS`；
   配置 URL 时用远程提供者，否则（或远程构造失败）回退 feature-hash
   （默认行为不变，R4 扩展安全门禁）。
4. **文档**：`ureq` 登记 `docs/DEPENDENCY_REGISTER.md`（Tier B）；
   L8 §2/§3.1 非目标与树更新；PROGRESS 0.1.70→0.1.71。

## Consequences

- 启用远程 embedding 后，向量值由外部服务决定（跨服务不可复现），
  但 `term_text` 投影仍确定性；语义索引布局（定维 + L2 归一）不变。
- ANN 索引是近似召回：召回率受 `bits`/`probes` 控制，测试以双向 top-5
  一致性与确定性门禁。
- 新增直接依赖 `ureq`（Tier B，已在 lock），`serde_json` 已登记；
  `scripts/audit-dependency-register.sh` 保持全绿。
- 默认（无环境变量）路径零行为变化：feature-hash 确定性 embedding +
  精确内存/持久索引。
