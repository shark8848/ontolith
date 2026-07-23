# Ontolith 任务进度台账

文档 ID: PROG-0001  
版本: 0.1.11  
状态: Active  
创建: 2026-07-15  
基准: [PLAN-0001](./Ontolith_Development_Plan.zh-CN.md)  
对照代码快照: 2026-07-23（L0–L5 全量实现分批提交完成 + CI/合规烟雾 + W3C 子集门禁 required-lite + strict 观测轨 + 文件审计 + systemd 打包；W3C 子集扩容至 must-pass 24/24；管理平台已纳入主干：`ontolith-management-server` + ACL 分离 + runtime probe + local/CI smoke + SLO 阈值门禁 + 窗口化 SLO 门禁；安全加固 ADR-0003 已起草）；Git 当前头: `main`（本波次收工提交后刷新）（COUNT+子查询+属性路径最小集 `+/*/|/^` 已收敛）

---

## 1. 使用说明

| 字段 | 含义 |
|------|------|
| 状态 | `未开始` / `进行中` / `部分完成` / `已完成` / `阻塞` |
| 完成度 | 相对该项计划范围的粗估百分比 |
| 证据 | 代码路径、测试、文档；无则写 `—` |
| 下次动作 | 当前最优先的一步 |

更新规则：

1. 每完成一个可验收增量，更新对应行状态与完成度。
2. 变更范围或优先级时，在 [§7 变更日志](#7-变更日志) 追加一条。
3. 阶段退出前，核对 [§4 里程碑退出标准](#4-里程碑-r1r4-退出标准) 是否全部勾选。
4. 有 ADR/RFC 时回填链接到对应 Phase/WBS 行。

图例：

- `[ ]` 未完成
- `[~]` 部分完成
- `[x]` 已完成
- `[-]` 本阶段不在范围 / 延期

---

## 2. 总览仪表盘

| 维度 | 状态 | 完成度 | 备注 |
|------|------|--------|------|
| 仓库与 crate 骨架 | 部分完成 | ~95% | 14 crate（+compliance）；Git 已有基线提交 |
| Phase 0 规划与治理 | 部分完成 | ~60% | 台账 + ADR/RFC 模板 + 依赖登记 + 计划互链；签批仍缺 |
| Phase 1 核心模型与存储抽象 | 部分完成 | ~70% | L0/L1 文档化；ConsistencyLevel；存储契约固化 |
| Phase 2 持久化与事务内核 | 部分完成 | ~85% | 内存六索引 + RocksDB 耐久；无真 MVCC / 纯 CF 扫描 |
| Phase 3 查询引擎 | 部分完成 | ~90% | Turtle/TriG + SPARQL 核心代数/优化/绑定 + COUNT 聚合基线（无 GROUP BY）+ 子查询基线（嵌套 SELECT + LIMIT）+ 属性路径最小集（`/`、`+`、`*`、`|`、`^`）+ W3C 子集门禁（required-lite，must-pass 24/24）+ strict 观测轨；缺属性路径 `?` / 完整聚合 / Update |
| Phase 4 集群与一致性 MVP | 部分完成 | ~80% | +session 粘性/quorum commit/partition/rebalance + L5 /cluster API；无多进程 Raft |
| Phase 5 接入层与安全基线 | 部分完成 | ~88% | HTTP 全路由 + 文件审计 + cluster 权限 + systemd 打包 + 独立管理服务器（配置/监控/数据管理）+ 管理 ACL + runtime probe；无 TLS/OIDC |
| Phase 6 推理与验证 | 未开始 | ~5% | 仅类型占位 |
| Phase 7 企业运维与发布 | 部分完成 | ~26% | GitHub Actions CI + 本地 ci-local + systemd 部署脚本（含 management server）+ 管理面 smoke 门禁；无发布/回滚 |
| Phase 8 AI-Native 扩展 | 未开始 | 0% | — |
| **分层内核 L0–L3** | **部分完成** | **~88–90%** | 语义+存储+查询主路径可用，COUNT/子查询/属性路径最小集已纳入回归保护 |
| **相对 R1 退出标准** | **进行中** | **~70–73%** | 内核+HTTP+集群+CI/烟雾合规 + W3C 子集 required-lite；多节点数据面/W3C 全量/SLO 仍缺 |
| **相对 R1–R4 全计划** | **进行中** | **~12–15%** | — |

### 架构分层完成度（实现视图）

| 层 | 完成度 | 状态 |
|----|--------|------|
| L0 core | ~85% | KO/Canonical/Error/ConsistencyLevel |
| L1 rdf | ~80% | Triple/Quad/Dataset |
| L2 storage/txn | ~85% | 内存+RocksDB |
| L3 parser/query | ~90% | 完整核心，非仅 MVP；COUNT 聚合+子查询+属性路径最小集（`/`、`+`、`*`、`|`、`^`）已落地；W3C 子集 required-lite + strict 观测双轨 |
| L4 cluster | ~80% | +session/partition/rebalance/commit + HTTP /cluster；14 测 |
| L5 server/security/obs | ~88% | 双后端、文件审计、Results JSON、ingest、增强指标、部署脚本、管理面二进制与管理 API + ACL + runtime probe |
| L6 reasoner | ~5% | 占位 |
| L7 平台工程 | ~26% | CI workflow + ci-local + compliance crate + systemd 安装脚本（runtime + management）+ 管理面 smoke 校验 |
| L8 AI-Native | 0% | — |

### 当前焦点

| 优先级 | 焦点 | 负责人 | 目标日期 |
|--------|------|--------|----------|
| P0 | 管理面安全加固落地（TLS 终止方案或 OIDC 校验链路） | TBD | 进行中 |
| P1 | 管理平台窗口化 SLO（天/周）自动化与性能基线（benchmarks/SLO） | TBD | 进行中 |
| P2 | 多进程 Raft ADR / openraft | TBD | TBD |

---

## 3. Phase 进度明细

### Phase 0 — 规划冻结与治理基线

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P0-01 | 已批准范围基线 | 未开始 | 0% | 计划仍为 Draft | 评审并签批 PLAN-0001 |
| P0-02 | 架构例外审批模板 | 已完成 | 100% | [adr/0000-template.md](../adr/0000-template.md) + ADR-0001/0002 | 按需新增 ADR |
| P0-03 | 依赖登记模板与评审规则 | 部分完成 | 70% | [DEPENDENCY_REGISTER.md](./DEPENDENCY_REGISTER.md) | 持续维护 + CI 审计 |
| P0-04 | RFC 流程落地 | 部分完成 | 40% | [rfc/0000-template.md](../rfc/0000-template.md) | 首个实质 RFC 试用 |
| P0-05 | 进度台账 | 已完成 | 100% | 本文档 | 按增量维护 |

**阶段退出条件：** P0-01～P0-04 均为已完成。

---

### Phase 1 — 核心模型与存储抽象

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P1-01 | Knowledge Object 领域模型 | 部分完成 | 70% | L0 KO + L1 Statement/Graph/Dataset；见 IMPL-L0/L1 文档 | Part II 序列化；Ontology 载荷联动 reasoner |
| P1-02 | Node 标识与字典管理器 | 部分完成 | 75% | 内存字典 + RocksDB 持久字典 | 并发字典契约文档 |
| P1-03 | 存储抽象接口 | 部分完成 | 90% | stats/matching/snapshot_with/delete 精确 API | 版本冻结说明 |
| P1-04 | 确定性标识与规范化编码规则 | 部分完成 | 90% | 六置换物理键 + triple/quad set key | 独立编码 RFC；磁盘布局 |

**阶段退出条件：** P1-01～P1-04 达到可被 Phase 2 依赖的稳定契约。

---

### Phase 2 — 持久化与事务内核

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P2-01 | RocksDB 适配（抽象层下） | 部分完成 | 80% | `RocksDbStorageEngine` + CF + ADR-0001 | 纯 CF 索引扫描；运维参数调优 |
| P2-02 | WAL / 快照恢复 / MVCC 基线 | 部分完成 | 75% | 内存+Rocks WAL CF、reopen 恢复、snapshot+consistency | 真 MVCC 版本链 |
| P2-03 | 三元组/四元组物理编码 | 部分完成 | 90% | codec + 六置换键 + CF 落盘 | 列族级索引键直接扫描 |
| P2-04 | 索引基线 SPO/POS/OSP | 部分完成 | 95% | 六置换增量（内存侧）+ GraphIndex + matching | 命名图六置换；Async 维护 |
| P2-05 | 可恢复耐久写入路径 | 部分完成 | 85% | RocksDB commit/reopen/delete 单测通过 | fsync 策略/备份演练 |
| P2-06 | 事务行为规范文档 | 部分完成 | 95% | [L2 文档 v3](./L2-ontolith-storage-transaction-kernel.md) | 随真 MVCC 修订 |

**阶段退出条件：** 耐久写入可恢复；至少 SPO/POS/OSP；事务文档发布。

---

### Phase 3 — 查询引擎 MVP

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P3-01 | SPARQL 解析到执行主链路 | 部分完成 | 91% | SELECT/ASK/CONSTRUCT + JOIN/OPT/UNION/FILTER/BIND/VALUES + COUNT 聚合（无 GROUP BY）+ 子查询基线（嵌套 SELECT + LIMIT）+ 属性路径最小集（`/`、`+`、`*`、`|`、`^`） | 属性路径 `?` / 完整聚合 |
| P3-02 | 规则优化基线 | 部分完成 | 55% | BGP 重排、Identity 消除、Filter 下推、POS/OSP 选路 | 代价模型/统计 |
| P3-03 | Explain 输出 | 部分完成 | 85% | logical/physical/algebra + optimize 步骤 | HTTP Explain API |
| P3-04 | 超时与取消 API | 部分完成 | 75% | timeout_ms + Arc\<AtomicBool\> cancel | 异步抢占/token |
| P3-05 | MVP 标准符合性子集 | 部分完成 | 90% | 引擎单测 + [ontolith-compliance](../crates/ontolith-compliance) 15 烟雾 + W3C 子集运行器（must-pass 24/24，known-gap xfail=0，unsupported skip=1，其中 Update 为 strict skip-exempt）+ CI required-lite + strict observer + strict-promotion-readiness 自动信号 + `ci-local.sh` 全链路通过 | 观察主干连续 3 次 CI 全绿后评估 strict required |

**阶段退出条件：** MVP profile 查询可跑通；Explain/超时/取消可用。

---

### Phase 4 — 集群与一致性 MVP

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P4-01 | 元数据服务与主从选举 | 部分完成 | 85% | membership/status + bootstrap + 分区感知选主 | 多进程 RPC |
| P4-02 | Raft 控制基线 | 部分完成 | 55% | 任期/日志 + **commit_index 多数派**；ADR-0002 | openraft |
| P4-03 | 单区域分片与复制 | 部分完成 | 85% | hash slot + lag + **rebalance** | 跨节点数据搬迁 |
| P4-04 | 故障转移基线 | 部分完成 | 85% | failover + **partition 注入/愈合** | 真实网络分区 |
| P4-05 | 读一致性级别与 API 说明 | 部分完成 | 95% | Session 粘性 + [L4 文档 v2](./L4-ontolith-cluster-consistency.md) + **/cluster HTTP** | — |

**阶段退出条件：** 单区域复制 + 选主/故障转移可演示。

---

### Phase 5 — 接入层与安全基线

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P5-01 | 网关与服务接入边界 | 部分完成 | 92% | 全路由 + memory/rocksdb 工厂 + SPARQL Results JSON + 独立 `ontolith-management-server` 管理面 + 健康探测 | TLS；gRPC |
| P5-02 | 鉴权 / 授权 | 部分完成 | 68% | Header/API-Key + Permission + `cluster:admin` + 管理面 read/write ACL | OIDC/JWT |
| P5-03 | 租户隔离 | 部分完成 | 55% | 审计租户过滤 + `tenant_graph` 写入命名图 | 强制分库/行级 |
| P5-04 | 审计日志 | 部分完成 | 80% | 内存 + `FileAuditLog` JSONL（`ONTOLITH_AUDIT_PATH`） | 哈希链/不可篡改 |
| P5-05 | 指标 / 追踪 / 日志基线 | 部分完成 | 82% | 延迟/状态码/错误计数 + access log + 管理面监控聚合视图（`/admin/monitoring`）+ runtime probe | Tracing 全链路 |

**阶段退出条件：** 安全基线挂在真实请求路径；统一遥测可用。

---

### Phase 6 — 推理与验证增强

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P6-01 | OWL 2 RL 核心规则 | 未开始 | 5% | 类型占位 | 规则引擎骨架 |
| P6-02 | SHACL 基线验证 | 未开始 | 0% | — | 约束子集定义 |
| P6-03 | 可配置推理模式与保护 | 未开始 | 10% | `InferenceMode` | 超时/迭代上限护栏 |

**阶段退出条件：** RL 核心 + SHACL 基线 + 性能护栏可配置。

---

### Phase 7 — 企业运维与发布工程化

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P7-01 | 在线重平衡与灾备演练 | 未开始 | 0% | — | 演练手册骨架 |
| P7-02 | 性能回归门禁与 SLO 看板 | 未开始 | 0% | `benchmarks/` 空 | 基线基准用例 |
| P7-03 | 发布流水线与回滚验证 | 部分完成 | 45% | [.github/workflows/ci.yml](../.github/workflows/ci.yml) + `scripts/ci-local.sh` + [L5-systemd-service.md](./L5-systemd-service.md) + runtime/management install scripts + 管理面 smoke + probe latency 阈值门禁 | 发布/回滚手册 |
| P7-04 | 运维手册与证据包 | 未开始 | 0% | — | 按阶段产出 |

**阶段退出条件：** CI 门禁、演练证据、发布/回滚手册齐备。

---

### Phase 8 — AI-Native 语义扩展

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P8-01 | 语义-向量桥接 | 未开始 | 0% | — | R4 启动时立项 |
| P8-02 | 检索增强接口 | 未开始 | 0% | — | — |
| P8-03 | 代理集成扩展点 | 未开始 | 0% | — | 可挂 plugin-api |

**阶段退出条件：** 扩展安全与兼容门禁通过。

---

## 4. 里程碑 R1–R4 退出标准

### R1 MVP

| 检查项 | 状态 | 备注 |
|--------|------|------|
| [~] RDF 核心运行时可验收 | 部分完成 | L0–L3 + 解析/存储闭环；缺正式验收包 |
| [~] SPARQL 查询基线 | 部分完成 | SELECT/ASK/CONSTRUCT 核心；非完整 1.1 |
| [~] 单区域集群核心 | 部分完成 | 控制面可测+HTTP 演示；无多节点数据面 |
| [~] 安全与审计基线 | 部分完成 | HTTP 鉴权+审计+JSONL 落盘；无 OIDC |
| [~] 标准符合性门禁通过 | 部分完成 | CI + R1 烟雾 15 测 + W3C 子集（required-lite，must-pass 24/24，xfail=0，xpass=0，skip=1，Update 为 strict skip-exempt）+ strict observer（non-blocking）+ strict readiness 自动评估；无完整 W3C 套件 |
| [ ] 核心 SLO 基线达标 | 未完成 | 无基准 |
| [~] 恢复演练通过 | 部分完成 | RocksDB reopen 单测；无演练手册 |
| [ ] 回滚演练通过 | 未完成 | 无发布链路 |

**R1 判定：** 未达退出标准（约 **67–70%**；内核+HTTP+集群控制面可演示，`ci-local.sh` 全链路通过并纳入 W3C 子集 required-lite + strict observer 双轨；多节点数据面/符合性全量/SLO 仍缺）。

### R2

| 检查项 | 状态 |
|--------|------|
| [ ] 代价优化 | 未开始 |
| [ ] OWL 2 RL 核心 | 未开始 |
| [ ] SHACL 基线 | 未开始 |
| [ ] Explain/优化稳定性门禁 | 未开始 |
| [ ] 推理正确性与性能护栏 | 未开始 |

### R3

| 检查项 | 状态 |
|--------|------|
| [ ] 高级集群运维 | 未开始 |
| [ ] GeoSPARQL 范围能力 | 未开始 |
| [ ] 企业安全加固 | 未开始 |
| [ ] HA/故障转移门禁 | 未开始 |
| [ ] 租户隔离与审计加固门禁 | 未开始 |

### R4

| 检查项 | 状态 |
|--------|------|
| [ ] AI-native 语义扩展 | 未开始 |
| [ ] 扩展安全与兼容门禁 | 未开始 |
| [ ] 检索与语义集成 KPI | 未开始 |

---

## 5. WBS 进度

| WBS | 名称 | 状态 | 完成度 | 主要缺口 |
|-----|------|------|--------|----------|
| WBS-01 | 核心运行时与知识模型 | 部分完成 | ~70% | L0+L1；Part II 序列化仍缺 |
| WBS-02 | 解析与导入 | 部分完成 | ~80% | N-T/N-Q/Turtle/TriG/流式；JSON-LD 未做 |
| WBS-03 | 存储与事务 | 部分完成 | ~85% | RocksDB 已接；真 MVCC / 纯 CF 扫描仍缺 |
| WBS-04 | 查询与优化 | 部分完成 | ~88% | 完整核心代数+优化+绑定 + COUNT 聚合基线 + 子查询基线 + 属性路径最小集（`/`、`+`、`*`、`|`、`^`）+ W3C 子集门禁；缺属性路径 `?` / 完整聚合 |
| WBS-05 | 推理与 SHACL | 未开始 | ~5% | 全部实现 |
| WBS-06 | 分布式运行时 | 部分完成 | ~75% | 控制面增强+HTTP；无多进程数据复制 |
| WBS-07 | API、安全与集成 | 部分完成 | ~85% | 双后端网关+文件审计+Results JSON+ingest+部署脚本+独立管理面 API + ACL/probe；无 TLS/OIDC |
| WBS-08 | 平台工程 | 部分完成 | ~31% | CI workflow + compliance crate + ci-local + systemd 运维文档 + 管理面 smoke + 窗口化 SLO 检查脚本；无发布回滚 |

---

## 6. 质量门禁与治理清单

| 门禁/治理项 | 状态 | 证据 / 缺口 |
|-------------|------|-------------|
| [~] RDF/SPARQL 标准测试 | 部分完成 | `ontolith-compliance` R1 烟雾 15 + W3C 子集运行器（must-pass 24/24，known-gap: xfail 0 / xpass 0，unsupported skip 1，Update 为 strict skip-exempt）+ CI required-lite / strict observer；非完整 W3C 官方 |
| [~] 故障注入（选主/复制/恢复） | 部分完成 | `ontolith-cluster` 分区注入/愈合与复制路径单测（14 测） |
| [ ] 幂等写入验证 | 未开始 | 部分事务单测不足替代 |
| [ ] 性能回归门禁 | 未开始 | `benchmarks/` 空 |
| [~] 鉴权与租户隔离测试 | 部分完成 | `ontolith-security` 7 测（enforced/tenant/user/audit）+ server tenant_graph 路径 |
| [~] 管理平台控制面回归门禁 | 部分完成 | `ontolith-server` 管理面单测（ACL/probe）+ CI/local smoke + latency 阈值门禁 + 短窗口 SLO 统计（success%/p95） |
| [ ] 许可证与漏洞审计 CI | 未开始 | — |
| [x] `cargo fmt` / `clippy -D warnings` CI | 已完成 | GitHub Actions + `scripts/ci-local.sh` |
| [x] 全量测试 CI | 已完成 | workspace + rocksdb-smoke job + 本地 `./scripts/ci-local.sh`（2026-07-22 通过） |
| [ ] Miri/sanitizer（敏感模块） | 未开始 | — |
| [~] Cargo.lock 可复现构建 | 部分完成 | lock 已有；第三方运行时依赖几乎未接入 |
| [x] Tier A 依赖 RFC/ADR | 已完成 | ADR-0001 RocksDB |
| [x] 依赖登记（owner/风险/回退） | 已完成 | DEPENDENCY_REGISTER.md |
| [x] 首次 Git 提交基线 | 已完成 | `main` @ `8d7eca1` → `origin/main`（含 docs + 13 crates + LICENSE） |
| [x] Tier A RocksDB ADR/登记 | 已完成 | ADR-0001 + DEPENDENCY_REGISTER |

### 已有测试资产（事实清单，非门禁通过）

| Crate | 测试覆盖概要 | 路径 |
|-------|--------------|------|
| ontolith-core | KO 生命周期、资源规范化、canonical 一致性（12 测） | `crates/ontolith-core/src/domain/mod.rs` |
| ontolith-rdf | term/triple/quad/dataset/canonical（11 测） | `crates/ontolith-rdf/src/domain/mod.rs` |
| ontolith-storage | 内存六索引 + RocksDB 耐久（reopen/abort/delete）+ codec（25 测） | `crates/ontolith-storage/src/infrastructure/**` |
| ontolith-transaction | begin/commit/abort、超时清理、active 上限、metrics（7 测） | `crates/ontolith-transaction/src/infrastructure/mod.rs` |
| ontolith-query | SELECT/ASK/CONSTRUCT、JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES、COUNT 聚合（无 GROUP BY）、子查询基线（嵌套 SELECT + LIMIT）、属性路径最小集（`/`、`+`、`*`、`|`、`^`）、Explain/timeout（30 测） | `crates/ontolith-query/src/infrastructure/**` |
| ontolith-parser | N-Triples/N-Quads/Turtle/TriG、流式事件、错误定位、Unsupported 格式（11 测） | `crates/ontolith-parser/src/infrastructure/**` |
| ontolith-cluster | 选主、分区、复制、commit、rebalance、session sticky（14 测） | `crates/ontolith-cluster/src/infrastructure/mod.rs` |
| ontolith-security | disabled/enforced、tenant/user、audit（内存+文件）（7 测） | `crates/ontolith-security/src/{application,infrastructure}/mod.rs` |
| ontolith-observability | sink、导出、采样循环、Prometheus 文本（6 测） | `crates/ontolith-observability/src/**` |
| ontolith-server | metrics、采样配置、HTTP query decode + 管理面 API/ACL/probe（15 测） | `crates/ontolith-server/src/{api,bootstrap,http,management}.rs` |
| ontolith-compliance | R1 烟雾 15 + W3C 子集 profile 1（must-pass 24/24） | `crates/ontolith-compliance/tests/**` |

---

## 7. 变更日志

| 日期 | 作者 | 变更 |
|------|------|------|
| 2026-07-15 | Claude Code | 初建台账 PROG-0001；基于 PLAN-0001 与工作区代码对照录入基线完成度 |
| 2026-07-17 | Claude Code | 移除 crates 嵌套 `.git`；提交 docs+crates 基线并推送 `origin/main`（`8d7eca1`） |
| 2026-07-17 | Claude Code | L0：`ontolith-core` 落地 SAS-0401 KO 基座（identity/resource/knowledge/canonical/error）；11 单测通过，下游 crate 回归绿 |
| 2026-07-17 | Claude Code | 新增 `docs/L0-...Foundation.md`；L1：`ontolith-rdf` Term/Triple/Quad/Dataset + KO 桥接 + canonical；11 单测；下游回归绿；`docs/L1-...Dataset.md` |
| 2026-07-17 | Claude Code | L2：SPO/POS/OSP 编码与内存索引、字典契约增强、StorageEngine 查询扩展；storage 24 测；`docs/L2-...kernel.md` |
| 2026-07-17 | Claude Code | L3：parser N-Triples/N-Quads；query SPARQL SELECT/ASK 子集 + algebra/explain/timeout；`docs/L3-...query.md` |
| 2026-07-17 | Claude Code | L3 完整化 v2：Turtle/TriG/流式错误；SPARQL JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES/CONSTRUCT/优化/Solution 绑定/cancel；parser11+query21 测 |
| 2026-07-17 | Claude Code | L2 v2：增量六索引、精确删/去重、GraphIndex、StorageStats、ConsistencyLevel、matching；L3 接入 matching；storage 30 测；L2 文档 v2 |
| 2026-07-17 | Claude Code | L2 v3 / P2-01：RocksDB 适配（CF、崩溃恢复、feature 门控）、ADR-0001、依赖登记；storage 35 测；L2 文档 v3 |
| 2026-07-17 | Claude Code | 进度回写：分层 L0–L8 仪表盘、R1 上修至 35–40%、焦点切 L5 HTTP 接入 |
| 2026-07-17 | Claude Code | L5：HTTP 网关 /sparql/explain/metrics/health/audit/data/nt；Header 鉴权+审计；server6+security5 测；L5 文档 |
| 2026-07-17 | Claude Code | L5 v2：EngineTripleRepository、RocksDB 切换、Turtle/TriG/NQ 写入、SPARQL Results JSON、/ready、增强 metrics/access log、tenant_graph；server 8 测 |
| 2026-07-17 | Claude Code | L4：InMemoryClusterRuntime（选主/分片/复制/failover/一致性路由）；10 测；ADR-0002；L4 文档 |
| 2026-07-17 | Claude Code | L4 v2：session 粘性、commit_index、partition、rebalance、ClusterStatus；L5 /cluster/*；cluster 14 + server 9 测 |
| 2026-07-17 | Claude Code | L5 systemd：user unit + install 脚本；release 二进制；服务 active @ 127.0.0.1:8090 |
| 2026-07-17 | Claude Code | 平台工程：ADR/RFC 模板、GitHub Actions CI、`scripts/ci-local.sh`、`ontolith-compliance` R1 烟雾 15、FileAuditLog 审计落盘、clippy 清零；R1 ~62–65% |
| 2026-07-22 | GitHub Copilot | 合规增量：新增 `sparql_w3c_subset`（must-pass/known-gap/unsupported 分类、strict 开关）、`tests/w3c/*` 子集样例、CI `w3c-subset` non-blocking 作业、本地 `ci-local.sh` 可选 required 模式；更新 `R1-sparql-smoke-compliance.md` |
| 2026-07-22 | GitHub Copilot | 提交序列整理：按模块分批提交 L0/L1（`2fd5ff7`）、L2/L3（`6173f45`）、L4（`c093b63`）、L5（`d322c05`）、治理文档（`3333ca4`），工作区 clean |
| 2026-07-22 | GitHub Copilot | 本地门禁复核：`./scripts/ci-local.sh` 通过（fmt/clippy/workspace tests/compliance smoke/W3C subset），W3C 子集 must-pass 10/10 |
| 2026-07-22 | GitHub Copilot | 启动并完成门禁晋升实现：`w3c-subset` 升级为 required-lite，新增 `w3c-subset-strict` non-blocking 观测作业；`ci-local.sh` 默认改为 required-lite 并兼容旧 strict 变量；修复 aggregate 误判（由“无断言 XPASS”改为带断言 known-gap）；本地 `./scripts/ci-local.sh` 全绿 |
| 2026-07-22 | GitHub Copilot | 底层优先增量：`ontolith-query` 落地 COUNT 聚合最小能力（无 GROUP BY），新增 query 测试 3 条（总计 24）；W3C 子集 `w3c-aggregate-gap` 晋升为 must-pass，统计更新为 must-pass 11/11、known-gap xfail 1、xpass 0、skip 2 |
| 2026-07-22 | GitHub Copilot | 底层优先增量：`ontolith-query` 落地嵌套 SELECT+LIMIT 子查询基线，新增 query 测试 1 条（总计 25）；W3C 子集 `w3c-subquery-gap` 晋升为 must-pass，统计更新为 must-pass 12/12、known-gap xfail 0、xpass 0、skip 2 |
| 2026-07-22 | GitHub Copilot | 底层优先增量：`ontolith-query` 落地属性路径序列（iri/iri）基线，新增 query 测试 1 条（总计 26）；W3C 子集 `w3c-property-path-unsupported` 晋升为 must-pass，统计更新为 must-pass 13/13、known-gap xfail 0、xpass 0、skip 1 |
| 2026-07-22 | GitHub Copilot | 底层优先增量：`ontolith-query` 完成属性路径高级算子最小集（`+`、`*`、`|`、`^`）并改为 `Path` 通用代数求值，新增 query 测试 4 条（总计 30）；W3C 子集新增 4 条路径 must-pass 用例并全绿，统计更新为 must-pass 17/17、known-gap xfail 0、xpass 0、skip 1 |
| 2026-07-22 | GitHub Copilot | 收工批次：完成高级属性路径最小集代码与合规/架构/进度文档同步，执行 `cargo test -p ontolith-query` 与 `cargo test -p ontolith-compliance` 全绿，进入提交封板。 |
| 2026-07-23 | GitHub Copilot | 合规扩容：W3C 子集新增 7 条 must-pass（ASK false、BGP JOIN 变体、VALUES tuple、DISTINCT+OFFSET、COUNT(*)、路径 `+/*` 变体），统计更新为 must-pass 24/24、known-gap xfail 0、xpass 0、skip 1；本地 `cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture` 全绿。 |
| 2026-07-23 | GitHub Copilot | CI 增量：新增 `sparql w3c strict promotion readiness` 作业（仅 main push），自动回看最近 3 次 strict observer 结果并输出 READY/NOT READY 信号；用于 strict required 晋升判据自动化。 |
| 2026-07-23 | GitHub Copilot | strict 策略优化：W3C 子集 strict 判据调整为“零 known-gap 失败 + 零 in-scope skip”，并将 `SPARQL Update` 标记为 strict skip-exempt，消除 out-of-scope 永久阻塞。 |
| 2026-07-23 | GitHub Copilot | L5 管理面增量：新增独立二进制 `ontolith-management-server`，提供 `/admin/config`、`/admin/monitoring`、`/admin/data/*` 统一管理接口；`cargo test -p ontolith-server` 10 测通过。 |
| 2026-07-23 | GitHub Copilot | 运维增量：新增 management server 的 systemd user/system unit、环境模板与安装脚本，补齐管理面部署路径与健康检查文档。 |
| 2026-07-23 | GitHub Copilot | 管理面权限增量：新增 read/write ACL 分离（`ONTOLITH_MANAGEMENT_READ_KEY` / `ONTOLITH_MANAGEMENT_WRITE_KEY` + `X-Ontolith-Management-Key`），将管理查询与变更权限解耦。 |
| 2026-07-23 | GitHub Copilot | 管理面监控增量：新增 runtime probe（探测 `ONTOLITH_BIND` TCP 连通性与延迟），并在 `/admin/health`、`/admin/monitoring` 输出 `runtime_probe`。 |
| 2026-07-23 | GitHub Copilot | 规划对齐增量：将管理平台正式纳入中英文 PLAN 与 PROGRESS 的 Phase/WBS/R1 叙述，并补充管理面后续优先级队列（SLO、TLS/OIDC、多进程集群）。 |
| 2026-07-23 | GitHub Copilot | 门禁增量：`scripts/ci-local.sh` 与 CI `check` 作业新增管理面 smoke（启动 `ontolith-management-server` 并校验 `/admin/health` 与 `runtime_probe`）。 |
| 2026-07-23 | GitHub Copilot | SLO 增量：新增管理平台独立 SLO 文档（`docs/L5-management-platform-slo.md`），并将 smoke 门禁升级为 `runtime_probe.reachable=true` + `latency_ms` 阈值校验。 |
| 2026-07-23 | GitHub Copilot | 安全治理增量：起草 ADR-0003（管理面最小安全基线，TLS-first / OIDC-ready 路径）。 |
| 2026-07-23 | GitHub Copilot | SLO 增量：新增 `scripts/check-management-slo-window.sh` 窗口检查脚本（success%/p95），并接入 local/CI 管理面 smoke；补充 management env 模板阈值参数。 |

---

## 8. 近期行动队列（可勾选）

### 本周建议

- [x] 建立 `docs/PROGRESS.md` 进度台账
- [x] 根仓库首次 commit（文档 + 骨架 + 现有实现）作为进度基线（`main` / `8d7eca1`）
- [x] 新增 ADR-0001（RocksDB）与依赖登记表
- [x] 新增通用 `adr/0000-template.md` / `rfc/0000-template.md`
- [x] 剩余实现按模块拆分为可审阅提交序列（L0/L1 → L5 + 治理文档）
- [x] 新增独立管理服务器（统一配置/监控/数据管理）并完成基础测试
- [x] 管理面 ACL 分离（read/write key）
- [x] 管理面 runtime probe（health/monitoring）
- [x] 管理面纳入规划与台账主线（PLAN + PROGRESS）
- [x] 管理面 SLO 基线（probe 成功率/延迟阈值）文档化并接入门禁判据
- [x] 管理面窗口化 SLO（success%/p95）与告警阈值固化
- [x] 管理面最小安全加固 ADR 草案（ADR-0003）
- [ ] 确认 Stream A/B/C/D 负责人并回填 §2 焦点表
- [ ] 将本地提交序列推送并发起 PR（附分批审阅说明）
- [ ] 管理面安全加固（TLS 终止方案落地或 OIDC 校验链路实现）

### R1 关键路径（按依赖序）

1. [~] Phase 0 签批与模板（模板齐；签批未做）
2. [~] Phase 1 KO 模型 + 存储契约文档
3. [~] Phase 2 RocksDB + 多索引 + 事务文档
4. [~] Phase 3 真 SPARQL MVP + Explain/超时 + R1 烟雾
5. [~] Phase 4 单区域集群最小闭环（可与 3 并行）
6. [~] Phase 5 网关 + 鉴权/租户/审计落盘（Tracing/OIDC 仍缺）
7. [ ] R1 退出标准全表勾选

---

## 9. 关联文档

- [开发计划（中文）](./Ontolith_Development_Plan.zh-CN.md)
- [Development Plan (EN)](./Ontolith_Development_Plan.md)
- [软件架构规范](./Ontolith_Software_Architecture_Specification.md)
- [SAS-0401 Knowledge Object Model](./SAS-0401%20—%20Knowledge%20Object%20Model.md)
- [架构手册目录](./Ontolith_Architecture_Handbook_Table_of_Contents.md)
- [L0 ontolith-core 功能说明](./L0-ontolith-core-Knowledge-Object-Foundation.md)
- [L1 ontolith-rdf 功能说明](./L1-ontolith-rdf-Statement-Graph-Dataset.md)
- [L2 storage/transaction 功能说明](./L2-ontolith-storage-transaction-kernel.md)
- [L3 parser/query 功能说明](./L3-ontolith-parser-query.md)
- [L5 access/security 功能说明](./L5-ontolith-access-security.md)
- [L5 管理平台 SLO 基线](./L5-management-platform-slo.md)
- [L4 cluster/consistency 功能说明](./L4-ontolith-cluster-consistency.md)
- [R1 SPARQL 烟雾符合性](./R1-sparql-smoke-compliance.md)
- [CI workflow](../.github/workflows/ci.yml) · [ci-local](../scripts/ci-local.sh)
- [ADR 模板](../adr/0000-template.md) · [RFC 模板](../rfc/0000-template.md)
