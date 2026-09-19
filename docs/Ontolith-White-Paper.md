# Ontolith 技术白皮书

**Ontolith: A Rust-First Semantic Runtime and Distributed Knowledge Graph Platform**
**以 Rust 为先的语义运行时与分布式知识图谱平台**

| 字段 | 值 |
|------|-----|
| 文档编号 | WP-0001 |
| 版本 | 1.2.0 |
| 状态 | Published |
| 发布日期 | 2026-09-19 |
| 项目 | Ontolith |
| 归属 | sharky-ai |
| 许可证 | GPL-3.0 |
| 基准文档 | [SAS-0001 架构规范](./Ontolith_Software_Architecture_Specification.md) · [PROG-0001 进度台账](./PROGRESS.md) |

---

## 摘要 (Executive Summary)

Ontolith 是一个用 Rust 编写的**云原生本体运行时与分布式语义计算平台**。它的使命，是成为本体管理、语义推理、分布式 RDF 存储以及标准兼容知识图谱基础设施的**参考级开源实现**。

与传统的三元组库或图数据库不同，Ontolith 从设计之初即确立了四条主张：

- **Standards First（标准优先）**：对外行为严格遵循 W3C 语义网标准（RDF 1.1、SPARQL 1.1、SHACL、GeoSPARQL）。
- **Reasoning Native（原生推理）**：前向链推理与 SHACL 数据形状校验作为一等公民内建于运行时。
- **Cloud Native（云原生）**：分层可替换架构 + 分布式数据面（多进程 Raft），面向水平扩展与在线运维。
- **Rust Powered（Rust 驱动）**：控制面与数据面全部以 Rust 实现，以类型系统与所有权模型换取内存安全与可验证的可靠性。

当前 Ontolith 已完成 **L0–L9 分层内核**的实现，覆盖 16 个 crate 的工作区，并在合规性上取得 **完整 W3C SPARQL 套件 492/492 全绿、SHACL 核心套件 98/98 全绿** 的成果，核心 SLO 实测达标（成功率 100%、P95=0ms）。R1–R4 全计划里程碑判定完成度约 100%，并已完成首次单节点生产发布与真实的发布/回滚演练。

---

## 1. 行业背景与挑战

### 1.1 知识图谱基础设施的结构性缺口

企业级知识图谱与语义数据管理长期面临三重矛盾：

1. **标准合规 vs. 工程落地**：许多系统宣称支持 SPARQL，但在 W3C 完整测试套件面前存在大量欠账；查询代数、属性路径、更新语义、聚合等高级特性的覆盖度参差不齐。
2. **数据安全性 vs. 性能**：语义引擎传统上构建在 JVM / 带垃圾回收的运行时之上，尾延迟与内存安全漏洞难以满足关键业务场景。
3. **单机能力 vs. 分布式演进**：从单机三元组库平滑演进到具备一致性保证的分布式集群，往往需要推倒重来。

### 1.2 Ontolith 的应对

Ontolith 以**分层可替换的 Rust 工作区**回应上述矛盾：把语义核心、存储事务内核、查询管线、集群一致性、接入安全、可观测性解耦为独立层次（L0–L9），每一层既可独立演进，又通过冻结的接口契约向上层提供稳定能力，从而实现"标准合规、内存安全、分布式就绪"三者的统一。

---

## 2. 设计原则

Ontolith 的架构决策受七条原则约束（源自 SAS-0001 §2）：

| 编号 | 原则 | 内涵 |
|------|------|------|
| AP-001 | Standards First | 对外行为 MUST 遵循 W3C 标准 |
| AP-002 | Modular Architecture | 每个子系统 SHALL 可独立替换 |
| AP-003 | Plugin First | 存储、解析器、序列化器、优化器、推理机、安全提供者 SHALL 可插拔 |
| AP-004 | Distributed by Design | 每个组件 SHALL 支持未来分布式部署 |
| AP-005 | Safety First | `unsafe` Rust 需显式架构评审批准 |
| AP-006 | Measurable Architecture | 架构决策 SHALL 附带可度量的验收标准 |
| AP-007 | Rust-Only Implementation | 生产控制面与数据面 MUST 全部以 Rust 实现 |

AP-007 尤其关键：非 Rust 代码仅允许用于构建/测试自动化、开发者工具与文档生成、由稳定 API 契约生成的互操作 SDK，以及通过 Rust 适配器调用的嵌入式第三方原生引擎。

---

## 3. 系统架构

### 3.1 分层模型

Ontolith 采用自底向上的分层内核，各层职责清晰、接口冻结：

```
┌─────────────────────────────────────────────────────────────┐
│  L9  GeoSPARQL 地理语义能力（scoped capability）             │
│  L8  AI-Native 语义检索 / 代理集成 / 远程 embedding          │
│  L7  运维：在线重平衡 · 灾备 · 发布/回滚 · SLO 门禁          │
│  L6  推理与验证：前向链推理 · SHACL 数据形状                  │
│  L5  接入与安全：HTTP/gRPC 网关 · 认证授权 · 审计 · 可观测   │
│  L4  集群与一致性：多进程 Raft 数据面 · 复制式元数据          │
│  L3  解析与查询：RDF 解析 · SPARQL 代数/优化/执行            │
│  L2  存储与事务内核：内存/RocksDB · MVCC · 事务生命周期      │
│  L1  RDF 值模型：Term / Triple / Quad / Graph / Dataset       │
│  L0  核心基础：知识对象模型 · 规范编码 · 共享错误             │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Crate 地图

| 层次 | Crate | 职责 |
|------|-------|------|
| L0 | `ontolith-core` | 身份/资源模型、规范编码、共享错误 |
| L1 | `ontolith-rdf` | Term/triple/quad/graph/dataset 值模型 |
| L2 | `ontolith-storage` | 存储抽象 + 内存/RocksDB 适配器 |
| L2 | `ontolith-transaction` | 事务生命周期与协调 |
| L3 | `ontolith-parser` | RDF 解析层（Turtle/TriG/RDF-XML） |
| L3 | `ontolith-query` | SPARQL 解析/优化/执行管线 |
| L4 | `ontolith-cluster` | 集群一致性与控制面原语（Raft 数据面） |
| L5 | `ontolith-server` | 访问边界与 HTTP/gRPC 网关 |
| L6 | `ontolith-reasoner` | 前向链推理 + SHACL 校验 |
| L8 | `ontolith-ai` | 语义检索、ANN 索引、远程 embedding |
| L9 | `ontolith-geo` | GeoSPARQL 能力 |
| 支撑 | `ontolith-security` | 认证上下文、授权、审计（哈希链） |
| 支撑 | `ontolith-observability` | 指标 / 追踪 / 日志模型 |
| 支撑 | `ontolith-plugin-api` | 插件边界与契约 |
| 支撑 | `ontolith-sdk` | 面向 SDK 的集成表面 |
| 支撑 | `ontolith-compliance` | SPARQL 烟雾 + W3C 子集测试框架 |

### 3.3 关键架构决策（ADR 摘要）

| ADR | 决策 |
|-----|------|
| ADR-0001 | 以 RocksDB 作为持久化存储后端 |
| ADR-0002 | 集群 MVP 采用进程内实现，逐步演进 |
| ADR-0003 | 管理平面安全最小集（TLS 终止、ACL 读写分离） |
| ADR-0004 | 多进程 Raft 数据面（openraft 背书选主/复制/提交） |
| ADR-0005 | GeoSPARQL 作为 scoped capability 落地 |
| ADR-0006 | 远程 embedding provider（外部 HTTP + 确定性 LSH 近似索引） |

---

## 4. 核心能力

### 4.1 RDF 数据建模与存储

- **值模型**：完整的 RDF 1.1 Term / Triple / Quad / Graph / Dataset 分层，严格区分布尔项（`"1"^^xsd:boolean` ≠ `true`）。
- **集合语义 API**：`NamedGraph` / `Dataset` 提供 set-semantics 写入与增删查（`insert_unique` / `contains_quad` / `remove_quad` / `dedup` / `merge_set`），成员判等统一基于规范编码字节，与追加型多重集 API 共存且不改变热路径布局。
- **IRI 校验**：基线启发式校验之外提供 opt-in 严格校验 API（`Iri::parse_strict` / `is_strict`，RFC 3986 §3.1 scheme 文法 + RFC 3987 §2.2 禁用字符集），供入口与导入门禁使用。
- **存储后端**：内存引擎 + 可选 RocksDB 耐久后端；磁盘 MVCC 版本链跨重启持久；耐久写入走 fsync，并具备 BackupEngine 备份/恢复与调度。
- **索引结构**：纯 CF 索引扫描（SPO / POS / OSP + 命名图 GSPO / GPOS / GOSP），配合 bloom filter / 块缓存 / 压缩调优，并支持 Async 索引维护（水位 + 后台追赶）。
- **字典与空间回收**：双向值↔id 字典持久化，epoch 语义与单调不重发的 id 分配；`gc_dictionary` 保守回收未被（存活语句 / MVCC 留存版本 / 在途事务 / blank-node 宾语）引用的字典条目，`vacuum` 对各数据 CF 物理压缩回收 tombstone。两者已上升为 `StorageEngine` 契约方法，并经管理面端点 `POST /admin/storage/gc-dictionary` / `POST /admin/storage/vacuum`（写 key + `cluster/admin` RBAC）对外运维，响应分别回报回收条数与压缩列族数。
- **幂等写入**：Put 集合语义去重、重放去重、重复 commit 拒绝、Delete 不存在为 no-op。

### 4.2 SPARQL 查询引擎

- **解析与执行**：Turtle/TriG + SPARQL 核心代数、优化器与绑定引擎。
- **完整聚合**：`GROUP BY` / `HAVING`、`COUNT(DISTINCT)` / `SUM` / `AVG` / `MIN` / `MAX`、子查询聚合。
- **属性路径**：`/`、`+`、`*`、`?`、`|`、`^` 最小完备集。
- **SPARQL Update**：`INSERT DATA` / `DELETE DATA` / `DELETE·INSERT…WHERE` / `DELETE WHERE` + 图管理 `ADD` / `COPY` / `MOVE` / `CREATE`（含 SILENT、USING/USING NAMED）。
- **数据集子句**：`FROM` / `FROM NAMED` / `USING` / `USING NAMED`（§18.2.1/§18.2.2 语义）。
- **合规基线**：完整 W3C `rdf-tests` sparql11 套件经 manifest 驱动 runner 达成 **492/492 全绿（fail=0、drift=0）**。

### 4.3 分布式一致性（L4）

- **多进程 Raft 数据面**：以 openraft 为共识内核，日志/状态机/快照由 Raft 背书；选主、epoch、复制日志、commit 自动化。
- **RPC 传输**：树内 HTTP/1.1 RPC（vote / append-entries / install-snapshot），共享 secret Bearer 认证，未引入重型框架。
- **持久化**：RocksDB 独立 `raft` 列族，重启后从持久化 applied 条目重建复制式节点注册表。
- **跨节点数据搬迁**：真实快照导出/导入 + 网络分区故障注入演练。
- **灾备演练**：真实 3 进程集群走完 选主→在线重平衡→复制收敛→杀 follower（多数派提交）→杀 leader（自动 failover）→重启追赶，`=== DRILL PASS ===`。

### 4.4 推理与验证（L6）

- **前向链推理引擎**：RDFS（rdfs5/6/7/8/9）、属性公理（prp-inv1/2、prp-symp、prp-fp/ifp、prp-key、属性链）、类公理（cax-sco、cls-svf/avf/int/uni/maxc2、hasValue）、等价（eq-sym/trans/rep）等规则集，带迭代上限与墙钟超时护栏，支持一致性 ⊥ 检测。
- **SHACL 数据形状校验**：目标/核心约束组件全齐，`sh:path` 支持 inverse/alternative/sequence/zeroOrMore/oneOrMore/zeroOrOne 属性路径表达式全量。
- **合规基线**：W3C SHACL 核心套件 **98/98 全绿**。

### 4.5 接入与安全（L5）

- **双网关**：HTTP（`ONTOLITH_BIND`，默认 `127.0.0.1:8080`）+ gRPC（`ONTOLITH_GRPC_BIND`，默认 `127.0.0.1:50051`，tonic + prost），共享同一执行路径与鉴权契约。
- **强制认证**：`ONTOLITH_AUTH_MODE=enforced` 下要求 `X-API-Key` / `X-Ontolith-Tenant` / `X-Ontolith-User`；跨租户访问返回 403。
- **租户隔离**：强制分库/行级隔离（`TenantMode` + `urn:tenant:<t>` 命名空间，越权引用 403）。
- **OIDC / JWT**：树内 HS256 验证（RFC 7519 子集 + 常量时间签名比对），完整 OIDC 链路（JWKS / RS256）。
- **管理面 TLS**：rustls 进程内终止；R2 门禁——非 loopback bind 无 TLS 拒绝启动。
- **审计**：文件审计 + SHA-256 哈希链，加密级哈希升级。
- **ACL 读写分离**：`ONTOLITH_MANAGEMENT_READ_KEY` / `ONTOLITH_MANAGEMENT_WRITE_KEY`。

### 4.6 可观测性（L5/L7）

- **全链路 Tracing**：W3C `traceparent` 解析/生成/延续，网关根 span + auth/execute/ingest 子 span，`GET /admin/traces` 列表。
- **指标与日志**：metrics/tracing/logging 统一模型；窗口化 SLO 门禁 + systemd timer 采集 + 告警策略（成功率 / 连续失败 / P95 / 尖峰）。

### 4.7 AI-Native 扩展（L8）

- **语义检索**：热路径 top-10 < 1ms（KPI 门禁背书）；RocksDB 持久化与增量更新。
- **代理集成**：plugin-api `Retrieval` 能力 + `AgentTool` 契约 + `SemanticRetrievalTool` 示例。
- **远程 embedding**：外部 HTTP provider + 确定性多探针 LSH 近似索引（ADR-0006）。

### 4.8 GeoSPARQL（L9）

以 scoped capability 形式（ADR-0005）提供地理语义查询能力，与核心 SPARQL 管线联动。

---

## 5. 技术栈与质量保障

### 5.1 技术选型

| 领域 | 选型 |
|------|------|
| 语言 | Rust（全工作区，rust-toolchain 固定） |
| 持久化 | RocksDB（可选后端） |
| 共识 | openraft（Raft） |
| gRPC | tonic 0.12 + prost 0.13 |
| TLS | rustls（进程内终止） |
| 管理控制台 | Vite SPA + 零依赖 Node API server |
| HTTPS 信任锚 | webpki-roots |

### 5.2 质量门禁

| 命令 | 作用 |
|------|------|
| `cargo build --workspace` | 构建整个工作区 |
| `cargo test --workspace --all-targets` | 全量测试 |
| `cargo fmt --all -- --check` | 格式检查 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 静态分析（零告警） |
| `./scripts/ci-local.sh` | 本地质量门禁 |
| `ONTOLITH_W3C_SUBSET_STRICT=1 ./scripts/ci-local.sh` | 严格子集门禁 |

CI 覆盖：GitHub Actions + 本地 ci-local + 存储微基准（bench 阈值断言硬门禁）+ 语义检索门禁 + license 审计 + 依赖登记审计 + cargo-audit CVE 观测。R1 正式验收包 G1–G5 全 PASS（workspace 20 test binary 全绿、W3C 492/492、SHACL 97/98→98/98）。

---

## 6. 部署运维

### 6.1 生产守护

`scripts/ontolith-prod-ctl.sh {start|stop|status|restart|logs}` 管理两个进程：
- **Gateway**（`ontolith-server`）：HTTP 8080 + gRPC 50051，RocksDB 后端。
- **Management**（`ontolith-management-server`）：管理 API 9091，含 runtime probe 与 ACL。

### 6.2 服务化

`deployments/` 与 `scripts/` 提供 user 级与 system 级 systemd 单元安装脚本（含 management server 与 SLO timers）。

### 6.3 管理控制台

`console/`（`node server.js`）提供多集群管理界面，默认监听 `127.0.0.1:8890`，通过 `clusters.json` 配置指向 gateway/management/gRPC 端点并注入每集群凭据。

### 6.4 快速自检

```bash
# 健康检查（enforced 模式需携带认证头）
curl -s -H "X-API-Key: <key>" -H "X-Ontolith-Tenant: prod" \
     -H "X-Ontolith-User: console" http://127.0.0.1:8080/health

# SPARQL 查询
curl -sG http://127.0.0.1:8080/sparql \
  --data-urlencode 'query=SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10'
```

---

## 7. 典型应用场景

- **企业级知识图谱平台**：以标准兼容的 RDF 存储 + SPARQL 查询 + 原生推理构建统一语义层。
- **数据治理与形状校验**：以 SHACL 核心套件对入图数据施加可验证的约束。
- **地理语义服务**：结合 GeoSPARQL 与推理规则构建地理事件本体查询。
- **AI/RAG 语义检索底座**：以 L8 语义检索 + 远程 embedding + ANN 索引为代理应用提供低延迟召回。
- **多租户 SaaS 语义后端**：强制租户隔离 + OIDC + 审计哈希链满足合规要求。

---

## 8. 项目现状与路线图

### 8.1 完成度概览

| 维度 | 状态 |
|------|------|
| 仓库与 crate 骨架（16 crate） | 已完成 ~100% |
| L0–L3 内核（语义/存储/查询） | 已完成 ~100%（收尾项与运维接线 2026-09-19 全部闭合：严格 IRI 校验、set-semantics API、字典 GC/vacuum 契约化 + 管理面端点） |
| L4 集群与一致性（多进程 Raft） | 已完成 ~100% |
| L5 接入与安全基线 | 已完成 ~100% |
| L6 推理与验证（SHACL 98/98） | 已完成 ~100% |
| L7 企业运维与发布 | 已完成 ~100% |
| L8 AI-Native | 已完成 ~100% |
| R1–R4 全计划 | 已完成 ~100% |

### 8.2 已交付里程碑

- **R1**：SPARQL 查询基线 + 单区域集群核心 + W3C 完整套件全绿 + 核心 SLO 达标 + 正式验收包。
- **R2**：双门禁 + OIDC 完整链路 + 管理面 TLS。
- **R3**：GeoSPARQL 范围能力 + 企业级安全加固 + HA/故障转移 + 租户隔离与审计门禁。
- **R4**：L8 全波次 + ACC-R4 验收包 ACCEPTANCE PASS。
- **首次生产发布**：REL-PROD-0001 单节点（RocksDB 持久 + AUTH enforced + 审计落盘）+ 真实发布/回滚演练 DRILL PASS。

### 8.3 后续演进方向

- L0–L3 内核已收敛至全量完成（含字典 GC/vacuum 的管理面接线）；后续仅为已声明的增强轨：RDF-star、Decimal 任意精度、IRI 完整百分号编码/host 语法（backlog），以及 set-semantics 哈希索引、统计直方图等性能轨。
- 多区域 active-active 语义（当前 SAS 规范中标记为 deferred）。
- 持续的 W3C / SHACL 合规欠账清零与性能基准演进。

---

## 9. 许可证与联系

- **许可证**：GPL-3.0，详见仓库根目录 LICENSE。
- **作者**：shark8848 · admin@sharky-ai.com
- **仓库文档**：架构规范 [SAS-0001](./Ontolith_Software_Architecture_Specification.md) · 开发计划 [PLAN-0001](./Ontolith_Development_Plan.md) · 进度台账 [PROG-0001](./PROGRESS.md) · 决策记录 [adr/](../adr/) · 提案草案 [rfc/](../rfc/)

---

> 本白皮书基于 Ontolith 代码仓库现状与既有规范（SAS-0001 v1.2.0）、进度台账（PROG-0001 v0.1.81）编制，反映截至 2026-09 的工程实现状态。
>
> 变更记录：1.0.0（2026-09-19）首版；1.1.0（2026-09-19）同步 L0–L3 内核收尾（§4.1 核心能力、§8.1 完成度概览、§8.3 演进方向）；1.2.0（2026-09-19）同步字典 GC/vacuum 提升为 `StorageEngine` 契约并接入管理面端点（§4.1、§8.1、§8.3）。