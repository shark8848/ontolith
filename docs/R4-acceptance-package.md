# R4 正式验收包 —— AI-native 语义运行时扩展

文档 ID: ACC-R4-0001
版本: 1.0.0
状态: Accepted（证据见 §5）
日期: 2026-08-09
对应: [PROGRESS.md](./PROGRESS.md) R4 退出标准、[PLAN-0001](./Ontolith_Development_Plan.zh-CN.md) §6 R4、
[L8-ai-native.md](./L8-ai-native.md)
执行入口: `bash scripts/acceptance-r4.sh`

---

## 1. 验收范围

R4 AI-native 语义运行时扩展 = L8 全波次交付：

- `ontolith-ai` crate：`EmbeddingProvider` 抽象 + 树内确定性
  `FeatureHashEmbedding`（FNV-1a 64 特征哈希，跨进程字节级一致）+ 可持久化
  `RocksSemanticIndex`（键=RFC-0001 `encode_term` 规范编码）+ `InMemorySemanticIndex`
  （扁平行主序矩阵 + `dot_const::<256>` 向量化热路径）+ `SemanticSearchService`。
- server 接线：`GET /semantic/search` / `POST /semantic/index` + 启动自动索引 +
  写回流（ingest 精确 ops 差异 / SPARQL Update 对账 / 位置无关引用检查），
  鉴权/审计复用 L5（`semantic:read/write`）。
- 检索 KPI 门禁：`ontolith-compliance/p802_retrieval_gate` + CI `retrieval-gates`
  作业 + 语义 bench 阈值/趋势。
- 代理集成扩展点：`ontolith-plugin-api` `PluginCapability::Retrieval` +
  `AgentTool` 契约 + `SemanticRetrievalTool` 示例工具。

本验收**不**覆盖：R3 后续轨（GeoSPARQL/企业级安全加固——见 R3 台账与对应门禁，
与 R4 同期推进但不属 ACC-R4 判据）。

## 2. 验收判据

| ID | 判据 | 阈值 | 证据 |
|----|------|------|------|
| G1 | 静态门禁：fmt / clippy | `fmt --check` 干净；clippy `-D warnings` 零告警 | 验收运行日志 |
| G2 | 全量测试 | `cargo test --workspace --all-targets` 全部 ok、0 FAILED | workspace-tests.log |
| G3 | 标准符合性零漂移 | `w3c11_suite` 492/492；`shacl_suite` 98/98（drift=0） | w3c11.log / shacl.log |
| G4 | 语义 HTTP 闭环 | 启用 semantic：`POST /semantic/index` 幂等、`GET /semantic/search` top-k 命中、同查询两次响应字节级一致、`/health` 暴露 semantic 姿态 | 验收运行日志 |
| G5 | 检索与语义集成 KPI | `p802_retrieval_gate`（release）3 测全绿：确定性/受控语料 top-1 相关命中/延迟 < 1ms；语义 bench 阈值断言 + 趋势 | kpi-gate.log / bench.log |
| G6 | 扩展安全与兼容门禁 | compliance 全门禁（r2_explain/r2_reasoner/p802/r3_geo/r3_security）通过；R4 扩展未造成 W3C/SHACL 漂移（G3） | gates.log |

全部 G1–G6 通过 ⇒ `=== ACCEPTANCE PASS ===`，判据达成。

## 3. 验收步骤

```bash
# 完整验收（含全量 workspace 测试与 W3C/SHACL 套件，耗时较长）
bash scripts/acceptance-r4.sh

# 快速复验（跳过 G2 全量测试，适合日常回归）
bash scripts/acceptance-r4.sh --skip-workspace-tests
```

脚本以随机端口 + `/tmp` 隔离目录运行，不触碰生产数据；结果汇总与日志输出到
`$ACCEPTANCE_EVIDENCE_DIR`（默认 `/tmp/ontolith-r4-acceptance-<pid>/`）。

## 4. 验收基线（2026-08-09 前已固化）

- `ontolith-ai`：16 测（embedding 确定性、索引幂等 upsert/remove、搜索 top-k、
  RocksDB 持久化重开、`SemanticRetrievalTool` 字节级确定性）。
- `ontolith-plugin-api`：4 测（`PluginCapability::Retrieval` + `AgentTool` 契约）。
- 检索 KPI 实测：10k 语料 `search_embedding` **0.33–0.52ms < 1ms**（优化前
  1.57–2.26ms，约 4–5x）；非 256 维回退路径 0.77ms。
- 扩展安全：语义端点鉴权/审计复用 L5 模式（`semantic:read/write`，未启用 501）。
- 兼容门禁：workspace 全量 + W3C 492/492 + SHACL 98/98 零漂移（L8 三波次提交均验证）。

## 5. 本次验收证据（2026-08-09）

运行：`bash scripts/acceptance-r4.sh`，证据目录 `/tmp/ontolith-r4-acceptance-<pid>/`
（`acceptance-summary.txt` + `workspace-tests.log` / `w3c11.log` / `shacl.log` /
`gates.log` / `kpi-gate.log` / `bench.log` / 服务日志）。

| ID | 结果 | 实测 |
|----|------|------|
| G1 | PASS | `cargo fmt --check` 干净；`cargo clippy --workspace --all-targets -- -D warnings` 零告警 |
| G2 | PASS | `cargo test --workspace --all-targets`：全 test binary ok、0 failed（含 ontolith-ai 16、plugin-api 4、cluster 31、W3C/SHACL 套件） |
| G3 | PASS | w3c11_suite `total=492 pass=492 fail=0 drift=0`；shacl_suite `total=98 pass=98 fail=0 drift=0` |
| G4 | PASS | semantic 启用后：`POST /semantic/index?term=urn:acc:semantic-term-1` → 幂等索引；`GET /semantic/search?q=semantic+term` → hits 含该 term；同查询两次响应字节级一致；`/health` `"semantic":"on"` |
| G5 | PASS | `p802_retrieval_gate`（release）：3 测全绿（字节级确定性 / 受控语料 top-1 相关命中 / 延迟 < 1ms）；语义 bench 阈值断言通过、趋势记录写入 |
| G6 | PASS | compliance 全门禁通过（r2_explain 5 / r2_reasoner 7 / p802 3 / r3_geo 5 / r3_security 3）；W3C/SHACL 零漂移（G3） |

## 6. 验收结论

- [x] G1 静态门禁通过
- [x] G2 全量测试通过
- [x] G3 标准符合性零漂移通过（W3C 492/492 + SHACL 98/98）
- [x] G4 语义 HTTP 闭环通过（索引/检索/确定性/健康姿态）
- [x] G5 检索与语义集成 KPI 通过（< 1ms 实测 + bench 阈值/趋势）
- [x] G6 扩展安全与兼容门禁通过

**结论：** R4 退出标准全部达成，`ACC-R4-0001` 验收通过（2026-08-09）。
遗留：无阻塞项；后续 AI-Native 演进（外部 EmbeddingProvider、RAG 管线）按
[L8-ai-native.md](./L8-ai-native.md) 里程碑与 R4 扩展安全门禁推进。

## 7. 引用

- [L8-ai-native.md](./L8-ai-native.md)（设计文档与里程碑）
- [PROGRESS.md](./PROGRESS.md) R4 检查项 / §8 L8 波次
- [PLAN-0001](./Ontolith_Development_Plan.zh-CN.md) §6 R4
- `crates/ontolith-ai` / `crates/ontolith-plugin-api` / `crates/ontolith-compliance`
