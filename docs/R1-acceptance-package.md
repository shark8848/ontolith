# R1 正式验收包 —— RDF 核心运行时可验收

文档 ID: ACC-R1-0001
版本: 1.0.0
状态: Accepted（证据见 §5）
日期: 2026-08-08
对应: [PROGRESS.md](./PROGRESS.md) R1 退出标准「RDF 核心运行时可验收」、
[Ontolith_Development_Plan.zh-CN.md](./Ontolith_Development_Plan.zh-CN.md) §6 R1 MVP
执行入口: `bash scripts/acceptance-r1.sh`

---

## 1. 验收范围

RDF 核心运行时 = L0 core（知识对象/Canonical/序列化）+ L1 RDF 模型 + L2 存储事务
内核（内存 MVCC / RocksDB 磁盘 MVCC）+ L3 SPARQL 解析与查询/更新引擎，经
`ontolith-server` HTTP `/sparql` 网关闭环（`INSERT DATA` → 持久化 → `SELECT` 读回）。

本次验收**不**覆盖：OIDC 完整链路（R2+ 轨）、L8 AI-Native（R4）、PLAN 签批等治理项。

## 2. 验收判据

| ID | 判据 | 阈值 | 证据 |
|----|------|------|------|
| G1 | 静态门禁：fmt / clippy | `fmt --check` 干净；clippy `-D warnings` 零告警 | 验收运行日志 |
| G2 | 全量测试 | `cargo test --workspace --all-targets` 全部 ok、0 FAILED | workspace-tests.log |
| G3 | 标准符合性 | `w3c11_suite` 492 PASS / 0 FAIL；`shacl_suite` 97 PASS / 1 FAIL（profile 锁定，drift=0） | w3c11.log / shacl.log |
| G4 | 运行时闭环（内存后端） | HTTP `INSERT DATA` 返回 `"affected":1`；`SELECT` 读回插入主语；`/health triples=1` | 验收运行日志 |
| G5 | 持久化闭环（RocksDB） | 写入后重启 reopen：`/health triples=1` 且 `SELECT` 读回数据 | 验收运行日志 |

全部 G1–G5 通过 ⇒ `=== ACCEPTANCE PASS ===`，判据达成。

## 3. 验收步骤

```bash
# 完整验收（含全量 workspace 测试，耗时较长）
bash scripts/acceptance-r1.sh

# 快速复验（跳过 G2 全量测试，适合日常回归）
bash scripts/acceptance-r1.sh --skip-workspace-tests
```

脚本以随机端口 + `/tmp` 隔离目录运行，不触碰生产数据；结果汇总与日志输出到
`$ACCEPTANCE_EVIDENCE_DIR`（默认 `/tmp/ontolith-r1-acceptance-<pid>/`）。

## 4. 验收基线（2026-08-08 前已固化）

- 全量测试资产：storage 51 测、cluster 31 测、SHACL 42 测、reasoner 80 测、
  server/security/observability 全绿（各轮提交已验证）。
- W3C SPARQL 套件 492/492（w3c11_profile.tsv 锁定，fail=0、drift=0）。
- W3C SHACL 核心套件 97/98（uniqueLang-002 词法差异 profile 锁定）。
- 核心 SLO 实测基线：20 样本 success 100%、p95=0ms、max=3ms（阈值 250ms）。

## 5. 本次验收证据（2026-08-08）

运行：`bash scripts/acceptance-r1.sh`，证据目录 `/tmp/ontolith-r1-acceptance-197024/`
（`acceptance-summary.txt` + `workspace-tests.log` / `w3c11.log` / `shacl.log` / 服务日志）。

| ID | 结果 | 实测 |
|----|------|------|
| G1 | PASS | `cargo fmt --check` 干净；`cargo clippy --workspace --all-targets -- -D warnings` 零告警 |
| G2 | PASS | `cargo test --workspace --all-targets`：20 个 test binary 全 ok、0 failed（合计 400 测，含 storage 51 / cluster 31 / server 44） |
| G3 | PASS | w3c11_suite `total=492 pass=492 fail=0 drift=0 missing=0`；shacl_suite `total=98 pass=97 fail=1 drift=0 missing=0`（uniqueLang-002 词法差异 profile 锁定） |
| G4 | PASS | 内存后端：`INSERT DATA { <urn:acc:s1> <urn:acc:p> "v1" }` → `affected=1`；`SELECT ?s WHERE { ?s <urn:acc:p> "v1" }` 返回 `{"type":"uri","value":"urn:acc:s1"}`；`/health triples=1` |
| G5 | PASS | RocksDB 写入 `urn:acc:r1` → 停服重启 reopen → `/health triples=1` 且 SELECT 读回 `urn:acc:r1` |

验收中发现的缺陷及修复（本次验收的附加值）：

- **HTTP 结果 JSON 渲染缺陷**：`/sparql` SELECT/CONSTRUCT 将存储的 IRI 主语一律渲染为
  `{"type":"bnode","value":"nN"}`（`BoundValue::Node` 未经字典解码）。根因：
  `app.rs bound_value_json`/CONSTRUCT 主语渲染未走 `DictionaryCodec::decode_node`
  （W3C 套件在进程内比较时已解码，故 492/492 未暴露；HTTP 输出暴露）。
  修复：`sparql_results_json`/`bound_value_json` 增字典参数，`BoundValue::Node` 按
  `decode_node` 判定 uri/bnode（与引擎 `node_id_term` 同语义），gRPC 调用点同步；
  新增回归测试 `sparql_http_json_renders_stored_iri_subject_as_uri`（server 43→44 测）。
- 验收前顺带将全仓对齐当前 rustfmt（stable 工具链 1.9.0）规范格式（24 文件，纯格式化）。

## 6. 验收结论

- [x] G1 静态门禁通过
- [x] G2 全量测试通过
- [x] G3 标准符合性通过（492/492、97/98）
- [x] G4 运行时闭环通过
- [x] G5 持久化闭环通过
- [x] 结论：RDF 核心运行时可验收 **达成**（签署：Codex，2026-08-08）

## 7. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-08-08 | 初版：验收范围/判据/步骤/证据模板（R1 正式验收包） |
| 2026-08-08 | 首次验收执行：G1–G5 全 PASS（400 测 / 492+97 套件 / 运行时与持久化闭环）；验收中发现并修复 HTTP 结果 JSON 的 IRI 主语渲染缺陷（server 43→44 测） |
