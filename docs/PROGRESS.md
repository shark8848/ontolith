# Ontolith 任务进度台账

文档 ID: PROG-0001  
版本: 0.1.19  
状态: Active  
创建: 2026-07-15  
基准: [PLAN-0001](./Ontolith_Development_Plan.zh-CN.md)  
对照代码快照: 2026-07-23（L0–L5 全量实现分批提交完成 + CI/合规烟雾 + W3C 子集门禁 required-lite + strict 观测轨 + 文件审计 + systemd 打包；W3C 子集扩容至 must-pass 24/24；管理平台已纳入主干：`ontolith-management-server` + ACL 分离 + runtime probe + local/CI smoke + SLO 阈值门禁 + 窗口化 SLO 门禁；安全加固 ADR-0003 已起草）；2026-08-06 增量：L0 序列化 Part II、L3 属性路径 `?`（W3C must-pass 25/25）+ RDF 序列化导出、L2 命名图六置换、L4 数据面同步、L5 审计哈希链、L6 前向链推理、L7 存储微基准与 CI bench/license 作业、**完整聚合（GROUP BY/HAVING + COUNT(DISTINCT)/SUM/AVG/MIN/MAX + 子查询聚合，W3C must-pass 27/27）**、**SPARQL Update（INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE，W3C must-pass 30/30、skip=0）**、**天/周窗口 SLO 自动化（systemd timer 采集/日周评估 + 告警策略：成功率/连续失败/P95/尖峰）**、**管理面 TLS 终止（rustls 进程内终止 + `ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY` + `/admin/config` TLS 姿态证据 + 自签证书脚本；ADR-0003 转 Accepted）**、**R2 非 loopback TLS 强制门禁（非 loopback bind 无 TLS 拒绝启动）**、**完整 W3C 套件接入（vendored `w3c/rdf-tests` sparql11 941 文件/28 feature + manifest 驱动 runner：QueryEvaluation/UpdateEvaluation/PositiveSyntax/NegativeSyntax + SRX/SRJ/TSV/CSV/Turtle/ASK 结果比对 + profile 锁定 492 条基线，127 PASS/365 FAIL 作为合规欠账）**、**Turtle 数字字面量词法修复（`.` 不再作为分隔符，完整 INTEGER/DECIMAL/DOUBLE 文法 + `.5` 前导点小数，parser 16→17 测）**；全量测试待本波提交前验证

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
5. 执行顺序遵循自底向上（L0→L8）：优先完成当前最低未完成层，再逐层向顶层应用推进；跨层依赖以被依赖层为准。

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
| Phase 1 核心模型与存储抽象 | 部分完成 | ~75% | L0/L1 文档化；ConsistencyLevel；存储契约固化；序列化 Part II（KO 二进制编解码） |
| Phase 2 持久化与事务内核 | 部分完成 | ~85% | 内存六索引 + RocksDB 耐久；无真 MVCC / 纯 CF 扫描 |
| Phase 3 查询引擎 | 部分完成 | ~96% | Turtle/TriG + SPARQL 核心代数/优化/绑定 + 完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）+ SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）+ 子查询基线 + 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ W3C 子集门禁（required-lite，must-pass 30/30）+ strict 观测轨 + **完整 W3C 套件 manifest 基线（492 条，127 PASS/365 FAIL）** |
| Phase 4 集群与一致性 MVP | 部分完成 | ~82% | +session 粘性/quorum commit/partition/rebalance + L5 /cluster API + 数据面同步接口（快照迁移入队/回执）；无多进程 Raft |
| Phase 5 接入层与安全基线 | 部分完成 | ~90% | HTTP 全路由 + 文件审计（含哈希链）+ cluster 权限 + systemd 打包 + 独立管理服务器（配置/监控/数据管理）+ 管理 ACL + runtime probe；无 TLS/OIDC |
| Phase 6 推理与验证 | 部分完成 | ~55% | 前向链推理引擎（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2（bnode 感知 + 同迭代检测），迭代上限 + 墙钟超时护栏）可用；**SHACL 基线校验引擎落地（目标/约束子集/逻辑形状/数值范围/属性对 + 验证报告，reasoner 4→52 测）** |
| Phase 7 企业运维与发布 | 部分完成 | ~33% | GitHub Actions CI + 本地 ci-local + systemd 部署脚本（含 management server）+ 管理面 smoke + 窗口化 SLO 门禁 + 存储微基准（CI bench 作业）+ license 审计 CI 作业；无发布/回滚 |
| Phase 8 AI-Native 扩展 | 未开始 | 0% | — |
| **分层内核 L0–L3** | **部分完成** | **~92–96%** | 语义+存储+查询主路径可用，完整聚合/Update/子查询/属性路径最小集（含 `?`）已纳入回归保护 |
| **相对 R1 退出标准** | **进行中** | **~78–82%** | 内核+HTTP+集群+CI/烟雾合规 + W3C 子集 required-lite（30/30）+ 完整 W3C 套件 manifest 基线接入（492 条，127 PASS/365 FAIL 欠账已 profile 化）；多节点数据面/SLO 仍缺 |
| **相对 R1–R4 全计划** | **进行中** | **~12–15%** | — |

### 架构分层完成度（实现视图）

| 层 | 完成度 | 状态 |
|----|--------|------|
| L0 core | ~90% | KO/Canonical/Error/ConsistencyLevel/序列化 Part II（20 测） |
| L1 rdf | ~80% | Triple/Quad/Dataset |
| L2 storage/txn | ~85% | 内存+RocksDB |
| L3 parser/query | ~96% | 完整核心，非仅 MVP；完整聚合 + SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）+子查询（含聚合）+属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ RDF 序列化导出；W3C 子集 required-lite（30/30）+ strict 观测双轨 + 完整 W3C 套件 manifest 基线 |
| L4 cluster | ~82% | +session/partition/rebalance/commit + HTTP /cluster + 数据面同步（快照迁移/回执）；17 测 |
| L5 server/security/obs | ~90% | 双后端、文件审计（哈希链）、Results JSON、ingest、增强指标、部署脚本、管理面二进制与管理 API + ACL + runtime probe |
| L6 reasoner | ~55% | 前向链推理引擎（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key（值→成员桶索引）、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2（bnode 感知 + 同迭代检测）、迭代上限 + 墙钟超时）+ SHACL 基线校验（目标四选 + 隐式类目标；class/datatype/nodeKind/min-max count/length/pattern(+flags)/in/hasValue/node/and/or/not/数值范围 min-max Inclusive/Exclusive/属性对 equals/disjoint/lessThan/lessThanOrEquals/qualifiedValueShape(+min/max count、disjoint)/closed(+ignoredProperties)；severity/message；ValidationReport）；52 测 |
| L7 平台工程 | ~33% | CI workflow + ci-local + compliance crate + systemd 安装脚本 + 管理面 smoke + 窗口化 SLO 校验 + 存储微基准 + license 审计作业 |
| L8 AI-Native | 0% | — |

### 当前焦点

| 优先级 | 焦点 | 负责人 | 目标日期 |
|--------|------|--------|----------|
| P0 | L0–L3 底层收尾（编码/字典契约文档、存储真 MVCC、查询代价模型与高级 Update、W3C 欠账提 PASS） | TBD | 进行中 |
| P1 | L4 集群多进程 Raft 实施（P4-02 M1–M3：openraft 适配 → 多进程 HTTP RPC + RocksDB raft CF → 默认运行时切换 + CI 三进程 smoke） | TBD | TBD |
| P2 | L5 应用层安全与隔离（P5-03 强制租户隔离、P5-02 OIDC/JWT、P5-05 Tracing） | TBD | TBD |
| P3 | L6 推理应用化（P6-01 规则扩展收尾 → P6-03 接入 server 查询/推理管线 → P6-02 SHACL 补全） | TBD | TBD |
| P4 | L7 运维演练与发布（P7-01/04 演练与运维手册、P7-02 阈值断言、P7-03 发布/回滚） | TBD | TBD |

---

## 3. Phase 进度明细

### Phase 0 — 规划冻结与治理基线

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P0-01 | 已批准范围基线 | 未开始 | 0% | 计划仍为 Draft | 评审并签批 PLAN-0001 |
| P0-02 | 架构例外审批模板 | 已完成 | 100% | [adr/0000-template.md](../adr/0000-template.md) + ADR-0001/0002 | 按需新增 ADR |
| P0-03 | 依赖登记模板与评审规则 | 部分完成 | 70% | [DEPENDENCY_REGISTER.md](./DEPENDENCY_REGISTER.md) | 持续维护 + CI 审计 |
| P0-04 | RFC 流程落地 | 部分完成 | 70% | [rfc/0000-template.md](../rfc/0000-template.md) + 首个实质 RFC [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)（编码/磁盘布局） | 评审 RFC-0001 并回填状态 |
| P0-05 | 进度台账 | 已完成 | 100% | 本文档 | 按增量维护 |

**阶段退出条件：** P0-01～P0-04 均为已完成。

---

### Phase 1 — 核心模型与存储抽象

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P1-01 | Knowledge Object 领域模型 | 部分完成 | 80% | L0 KO + L1 Statement/Graph/Dataset + 序列化 Part II（`KoCodec` 全容器往返）；见 IMPL-L0 文档 | Ontology 载荷联动 reasoner |
| P1-02 | Node 标识与字典管理器 | 部分完成 | 90% | 内存字典 + RocksDB 持久字典 + 并发字典契约（[L2-storage-contracts.md](./L2-storage-contracts.md) Part A） | 随 P2-02 MVCC 复核字典 epoch 语义 |
| P1-03 | 存储抽象接口 | 部分完成 | 95% | stats/matching/snapshot_with/delete 精确 API + 接口版本冻结 0.1.0（[L2-storage-contracts.md](./L2-storage-contracts.md) Part B） | 破坏性变更走 RFC/ADR 登记 |
| P1-04 | 确定性标识与规范化编码规则 | 部分完成 | 95% | 六置换物理键 + triple/quad set key + 编码规则/磁盘布局定稿（[RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)） | 磁盘布局随 MVCC（P2-02）演进时升级 |

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
| P3-01 | SPARQL 解析到执行主链路 | 部分完成 | 97% | SELECT/ASK/CONSTRUCT + JOIN/OPT/UNION/FILTER/BIND/VALUES + 完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）+ SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）+ 子查询基线（嵌套 SELECT + LIMIT）+ 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`） + RDF 序列化导出 | 高级 Update（LOAD/CLEAR/WITH 图作用域） |
| P3-02 | 规则优化基线 | 部分完成 | 55% | BGP 重排、Identity 消除、Filter 下推、POS/OSP 选路 | 代价模型/统计 |
| P3-03 | Explain 输出 | 部分完成 | 85% | logical/physical/algebra + optimize 步骤 | HTTP Explain API |
| P3-04 | 超时与取消 API | 部分完成 | 75% | timeout_ms + Arc\<AtomicBool\> cancel | 异步抢占/token |
| P3-05 | MVP 标准符合性子集 | 部分完成 | 97% | 引擎单测 + [ontolith-compliance](../crates/ontolith-compliance) 17 烟雾 + W3C 子集运行器（must-pass 30/30，known-gap xfail=0，unsupported skip=0）+ **完整 W3C 套件 manifest 驱动 runner（`w3c11_suite`，492 条基线，127 PASS/365 FAIL 欠账 profile 化防回归）** + CI required-lite + strict observer + strict-promotion-readiness 自动信号 + `ci-local.sh` 全链路通过 | 观察主干连续 3 次 CI 全绿后评估 strict required；按 profile 欠账逐项提 PASS |

**阶段退出条件：** MVP profile 查询可跑通；Explain/超时/取消可用。

---

### Phase 4 — 集群与一致性 MVP

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P4-01 | 元数据服务与主从选举 | 部分完成 | 85% | membership/status + bootstrap + 分区感知选主 | 多进程 RPC |
| P4-02 | Raft 控制基线 | 部分完成 | 60% | 任期/日志 + **commit_index 多数派**；ADR-0002 模拟器；**多进程 Raft 设计定稿 [ADR-0004](../adr/0004-multi-process-raft-data-plane.md)**（openraft behind traits，M1–M3） | M1 单节点 openraft 适配 |
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
| P5-04 | 审计日志 | 部分完成 | 90% | 内存 + `FileAuditLog` JSONL（`ONTOLITH_AUDIT_PATH`）+ 哈希链（`prev`/`hash` + `verify_chain`） | 加密级哈希升级（可选） |
| P5-05 | 指标 / 追踪 / 日志基线 | 部分完成 | 82% | 延迟/状态码/错误计数 + access log + 管理面监控聚合视图（`/admin/monitoring`）+ runtime probe | Tracing 全链路 |

**阶段退出条件：** 安全基线挂在真实请求路径；统一遥测可用。

---

### Phase 6 — 推理与验证增强

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P6-01 | OWL 2 RL 核心规则 | 部分完成 | 85% | `ForwardChainReasoner`：rdfs5/6/7/8/9 + prp-inv1/2 + prp-symp + prp-trp + prp-fp（功能属性→值 sameAs）+ prp-ifp（逆功能属性→主词 sameAs）+ cax-sco + cls-svf1/2 + cls-avf + cls-int1/2 + cls-uni + cls-maxc2（maxCardinality 1 → 值 sameAs）+ eq-sym/trans + eq-rep-s/p/o（sameAs 主/谓/宾替换）+ prp-key（owl:hasKey 列表键共享值→sameAs，值→成员桶索引）+ 一致性 ⊥ 检测（cax-dw/cls-com/cls-nothing1/2/eq-diff1/2，`ReasoningReport.inconsistent` 标记；bnode 感知 + 同迭代 frontier 检测）（含 bnode 限定词/列表表达式）、迭代闭包、`InferenceMode` 开关、30 测 | 扩展规则集（cls-hv1/2 hasValue、prp-irp/cax-adc/eq-diff2/3 AllDifferent、prp-chain 属性链） |
| P6-02 | SHACL 基线验证 | 部分完成 | 75% | `ShaclEngine`（[infrastructure/shacl.rs](../crates/ontolith-reasoner/src/infrastructure/shacl.rs)）：形状解析（节点/属性形状、`sh:property` 嵌套、RDF 列表 `sh:in`/`sh:and`/`sh:or`/`sh:ignoredProperties`）、目标选择（targetClass/targetNode/targetSubjectsOf/targetObjectsOf + sh:class 隐式目标）、约束子集（class/datatype/nodeKind/minCount/maxCount/minLength/maxLength/pattern(+flags)/in/hasValue/node/and/or/not/closed + 数值范围 min-max Inclusive/Exclusive + 属性对 equals/disjoint/lessThan/lessThanOrEquals）、属性形状参数（qualifiedValueShape + qualifiedMinCount/qualifiedMaxCount/qualifiedValueShapesDisjoint、ignoredProperties 并入 closed 白名单）、severity/message、`ValidationReport`（conforms 仅 Violation 判定不合规）；`sh:pattern` 内置小正则子集（全串匹配，无分组/交替，flags 支持 i）；21 测（reasoner 共 25） | 其余约束组件（`sh:languageIn` 等，需语言标签管道重构）与 W3C SHACL 套件接入（需网络） |
| P6-03 | 可配置推理模式与保护 | 部分完成 | 60% | `InferenceMode` + `max_iterations` 迭代上限 + `max_elapsed_ms` 墙钟超时护栏（`ReasoningReport.timed_out` 标记早停） | 接入 server 查询/推理管线 |

**阶段退出条件：** RL 核心 + SHACL 基线 + 性能护栏可配置。

---

### Phase 7 — 企业运维与发布工程化

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P7-01 | 在线重平衡与灾备演练 | 未开始 | 0% | — | 演练手册骨架 |
| P7-02 | 性能回归门禁与 SLO 看板 | 部分完成 | 25% | `crates/ontolith-storage/benches/storage_bench.rs` + `benchmarks/README.md` + CI `bench` 作业（冒烟基准） | 阈值断言/趋势记录 |
| P7-03 | 发布流水线与回滚验证 | 部分完成 | 48% | [.github/workflows/ci.yml](../.github/workflows/ci.yml) + `scripts/ci-local.sh` + [L5-systemd-service.md](./L5-systemd-service.md) + runtime/management install scripts + 管理面 smoke + probe latency 阈值门禁 + license 审计 CI 作业 | 发布/回滚手册 |
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
| [~] 单区域集群核心 | 部分完成 | 控制面可测+HTTP 演示；多节点数据面设计已定（[ADR-0004](../adr/0004-multi-process-raft-data-plane.md)），M1–M3 实施中 |
| [~] 安全与审计基线 | 部分完成 | HTTP 鉴权+审计+JSONL 落盘；无 OIDC |
| [~] 标准符合性门禁通过 | 部分完成 | CI + R1 烟雾 17 测 + W3C 子集（required-lite，must-pass 30/30，skip=0）+ strict observer（non-blocking）+ strict readiness 自动评估 + **完整 W3C 套件 manifest 基线（492 条，127 PASS/365 FAIL 已 profile 化防回归）**；PASS 份额提升仍在进行 |
| [ ] 核心 SLO 基线达标 | 未完成 | 无基准 |
| [~] 恢复演练通过 | 部分完成 | RocksDB reopen 单测；无演练手册 |
| [ ] 回滚演练通过 | 未完成 | 无发布链路 |

**R1 判定：** 未达退出标准（约 **67–70%**；内核+HTTP+集群控制面可演示，`ci-local.sh` 全链路通过并纳入 W3C 子集 required-lite + strict observer 双轨；多节点数据面/符合性全量/SLO 仍缺）。

### R2

| 检查项 | 状态 |
|--------|------|
| [ ] 代价优化 | 未开始 |
| [ ] OWL 2 RL 核心 | 未开始 |
| [~] SHACL 基线 | 部分完成（`ShaclEngine` 约束子集 + 逻辑形状 + qualified 计数 + ValidationReport 已落地；W3C SHACL 套件未接） |
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
| WBS-01 | 核心运行时与知识模型 | 部分完成 | ~78% | L0+L1 + 序列化 Part II（`KoCodec`）；Statement KO 挂载仍简 |
| WBS-02 | 解析与导入 | 部分完成 | ~85% | N-T/N-Q/Turtle/TriG/流式 + 序列化导出（N-T/N-Q）；JSON-LD 未做 |
| WBS-03 | 存储与事务 | 部分完成 | ~85% | RocksDB 已接；真 MVCC / 纯 CF 扫描仍缺 |
| WBS-04 | 查询与优化 | 部分完成 | ~95% | 完整核心代数+优化+绑定 + 完整聚合（GROUP BY/HAVING）+ SPARQL Update 基线 + 子查询（含聚合）+ 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ W3C 子集门禁（30/30）；缺高级 Update 形态 |
| WBS-05 | 推理与 SHACL | 部分完成 | ~18% | 前向链推理最小集可用；SHACL 基线校验（约束子集 + 逻辑形状 and/or/not + qualifiedValueShape 计数 + closed/ignoredProperties）落地 |
| WBS-06 | 分布式运行时 | 部分完成 | ~78% | 控制面增强+HTTP + 数据面同步接口；无多进程数据复制 |
| WBS-07 | API、安全与集成 | 部分完成 | ~87% | 双后端网关+文件审计（哈希链）+Results JSON+ingest+部署脚本+独立管理面 API + ACL/probe；无 TLS/OIDC |
| WBS-08 | 平台工程 | 部分完成 | ~35% | CI workflow + compliance crate + ci-local + systemd 运维文档 + 管理面 smoke + 窗口化 SLO 检查 + 存储微基准 + license 审计；无发布回滚 |

---

## 6. 质量门禁与治理清单

| 门禁/治理项 | 状态 | 证据 / 缺口 |
|-------------|------|-------------|
| [~] RDF/SPARQL 标准测试 | 部分完成 | `ontolith-compliance` R1 烟雾 17 + W3C 子集运行器（must-pass 30/30，skip=0）+ **完整 W3C 套件 manifest 驱动 runner（`w3c11_suite`，492 条基线：127 PASS / 365 FAIL，drift 防回归）** + CI required-lite / strict observer |
| [~] 故障注入（选主/复制/恢复） | 部分完成 | `ontolith-cluster` 分区注入/愈合与复制路径单测（14 测） |
| [ ] 幂等写入验证 | 未开始 | 部分事务单测不足替代 |
| [~] 性能回归门禁 | 部分完成 | `storage_bench` 微基准（字典/写入提交/索引匹配）+ CI `bench` 冒烟作业；阈值断言未接 |
| [~] 鉴权与租户隔离测试 | 部分完成 | `ontolith-security` 9 测（enforced/tenant/user/audit/哈希链）+ server tenant_graph 路径 |
| [~] 管理平台控制面回归门禁 | 部分完成 | `ontolith-server` 管理面单测（ACL/probe）+ CI/local smoke + latency 阈值门禁 + 短窗口 SLO 统计（success%/p95） |
| [~] 许可证与漏洞审计 CI | 部分完成 | CI `license-audit` 作业（`cargo-license` 枚举）；漏洞审计未接 |
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
| ontolith-core | KO 生命周期、资源规范化、canonical 一致性、序列化 Part II 往返/损坏拒绝（20 测） | `crates/ontolith-core/src/domain/mod.rs` |
| ontolith-rdf | term/triple/quad/dataset/canonical（11 测） | `crates/ontolith-rdf/src/domain/mod.rs` |
| ontolith-storage | 内存六索引 + 命名图六置换 + RocksDB 耐久（reopen/abort/delete）+ codec + 命名图匹配（30 测） | `crates/ontolith-storage/src/infrastructure/**` |
| ontolith-transaction | begin/commit/abort、超时清理、active 上限、metrics（7 测） | `crates/ontolith-transaction/src/infrastructure/mod.rs` |
| ontolith-query | SELECT/ASK/CONSTRUCT、JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES、完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）、SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）、子查询基线、属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）、Explain/timeout（46 测） | `crates/ontolith-query/src/infrastructure/**` |
| ontolith-parser | N-Triples/N-Quads/Turtle/TriG、流式事件、错误定位、Unsupported 格式、RDF 序列化导出、Turtle 数字字面量完整文法（17 测） | `crates/ontolith-parser/src/infrastructure/**` |
| ontolith-cluster | 选主、分区、复制、commit、rebalance、session sticky、数据面同步（17 测） | `crates/ontolith-cluster/src/infrastructure/mod.rs` |
| ontolith-security | disabled/enforced、tenant/user、audit（内存+文件）+ 哈希链验证/篡改检测（9 测） | `crates/ontolith-security/src/{application,infrastructure}/mod.rs` |
| ontolith-observability | sink、导出、采样循环、Prometheus 文本（6 测） | `crates/ontolith-observability/src/**` |
| ontolith-server | metrics、采样配置、HTTP query decode + 管理面 API/ACL/probe（15 测） | `crates/ontolith-server/src/{api,bootstrap,http,management}.rs` |
| ontolith-reasoner | 前向链推理（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2、`InferenceMode` 开关 + 迭代/超时护栏 + `ReasoningReport.inconsistent`）+ SHACL 基线校验（目标选择/约束子集/逻辑形状/数值范围/属性对/qualified 计数/报告，52 测） | `crates/ontolith-reasoner/src/infrastructure/{mod,shacl}.rs` |
| ontolith-compliance | R1 烟雾 17 + W3C 子集 profile 1（must-pass 30/30，skip=0）+ 完整 W3C 套件 manifest runner（`w3c11_suite`，492 条基线 profile 锁定） | `crates/ontolith-compliance/tests/**` |

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
| 2026-07-23 | GitHub Copilot | 收工快照：提交 `14ac4a7`、`cd098db` 已推送至 `origin/main`；本地 `scripts/ci-local.sh`（含 management windowed SLO）通过，工作区与远端同步。 |
| 2026-08-06 | Codex | 文档审计：依据 PLAN/PROGRESS/ADR 中的“进行中 / 草案 / 未完成”描述整理未完成项待办清单（§8），覆盖规划签批、架构定稿、ADR-0003 落地、天/周 SLO 与性能基线、R1 退出标准剩余项。 |
| 2026-08-06 | Codex | L0：序列化 Part II（`domain/serialization.rs`：确定性二进制 `KoCodec`，KO 容器全量往返、损坏拒绝），core 20 测。 |
| 2026-08-06 | Codex | L3：RDF 序列化导出（N-Triples/N-Quads 写出），parser 16 测；属性路径 `?`（zero-or-one，修饰符紧贴 IRI 消歧），query 32 测，W3C 子集 must-pass 24→25。 |
| 2026-08-06 | Codex | L2：命名图六置换（`GraphIndex` 位置索引 + `matching_in_named_graphs`），并接入 `StorageEngine::quads_matching_in_graph`（RocksDB 覆盖），storage 30 测。 |
| 2026-08-06 | Codex | L4：数据面同步（`DataPlaneSync`：快照迁移入队/`drain_syncs`/`SyncReceipt`），cluster 17 测；L5：审计哈希链（FNV-1a 64，`verify_chain` 全链校验），security 9 测。 |
| 2026-08-06 | Codex | L6：前向链推理引擎（rdfs5/6/7/8/9 + prp-inv1，`max_iterations` 护栏），reasoner 4 测；L7：存储微基准（`storage_bench` + `benchmarks/README.md`）+ CI `bench` / `license-audit` 作业。全量测试 183 通过。 |
| 2026-08-06 | Codex | L3：完整聚合落地——解析器支持投影聚合表达式（COUNT/SUM/AVG/MIN/MAX、COUNT(DISTINCT)）与 GROUP BY（含表达式别名）/HAVING（聚合重写为投影别名，支持 `SUM(?v) > n` 形式），子查询同步支持；执行器按组求值并施加 HAVING；query 32→39 测，W3C 子集 must-pass 25→27/27，全量测试 190 通过。 |
| 2026-08-06 | Codex | L3：SPARQL Update 落地——解析 `INSERT DATA` / `DELETE DATA` / `DELETE·INSERT…WHERE` / `DELETE WHERE`（LOAD/CLEAR/WITH 明确 Unsupported）；`QueryPlan.update_ops` + `QueryResult.affected`；`UpdateWriteService`（存储引擎写面）与 `UpdateQueryExecutor`（读委托 + 写事务，含字典 IRI→NodeId 桥）；server `/sparql` 接入写管线并渲染 update 结果；query 39→46 测，R1 烟雾 15→17，W3C 子集 must-pass 27→30/30 且 skip=0，全量测试 199 通过。 |
| 2026-08-06 | Codex | docs：中英文开发计划同步勾选「天/周窗口 SLO 自动化与告警」项（PLAN-0001 §R1 退出标准 / P1）。 |
| 2026-08-06 | Codex | L5：天/周窗口 SLO 自动化与告警策略——`collect-slo-sample.sh` 持久化 runtime_probe 样本（samples.jsonl）；`check-slo-window-history.sh` 窗口评估（成功率/P95/连续失败/延迟尖峰）+ `--self-test` 四用例 + reports/alerts 落盘；systemd user timers（5min 采集、每日 24h、每周 168h 评估）与安装脚本；接入 ci-local。 |
| 2026-08-06 | Codex | L5：管理面 TLS 终止与 R2 门禁（ADR-0003 转 Accepted）——`TlsServerConfig`/`HttpServer::with_tls`（rustls 进程内终止，PEM 加载，close_notify 冲刷）；`ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY`；`enforce_tls_gate` 非 loopback bind 无 TLS 拒绝启动；`/admin/config` 暴露 `tls` 姿态；`gen-self-signed-cert.sh` + env 示例；server 16→21 测，全量测试 205 通过。 |
| 2026-08-06 | Codex | L3：完整 W3C 套件接入——vendored 官方 `w3c/rdf-tests` sparql11（941 文件/28 feature，QueryEvaluation/UpdateEvaluation/PositiveSyntax/NegativeSyntax 四类）；manifest 驱动 runner `w3c11_suite.rs`（自有 Turtle 解析官方 manifest、SRX/SRJ/TSV/CSV + Turtle 图 + ASK 结果比对、超时/panic 护栏）；`w3c11_profile.tsv` 锁定 492 条基线（127 PASS / 365 FAIL，按 reason-code 分类），普通模式 drift 防回归、`ONTOLITH_W3C11_LEARN=1` 重生成；顺带修复 Turtle 数字字面量词法 bug（`.` 不再当分隔符，完整 INTEGER/DECIMAL/DOUBLE 文法 + `.5` 前导点），parser 16→17 测。 |
| 2026-08-06 | Codex | L4：多进程 Raft 设计定稿（[ADR-0004](../adr/0004-multi-process-raft-data-plane.md) 转 Accepted）：openraft 共识引擎 behind 现有 cluster traits；树内 axum/reqwest HTTP RPC（`/internal/raft/*` + 共享 secret）；RocksDB 独立 `raft` CF 存日志/硬状态/快照；写入经多数派提交后落 L2；保留 `InMemoryClusterRuntime` 作测试 harness；DEPENDENCY_REGISTER 与 L4 文档同步。 |
| 2026-08-06 | Codex | L6：SHACL 基线校验引擎——`ShaclEngine`（`ShaclValidator` trait behind）：形状解析（节点/属性形状、`sh:property` 嵌套、RDF 列表 `sh:in`）；目标选择（targetClass/targetNode/targetSubjectsOf/targetObjectsOf + `sh:class` 隐式类目标）；约束子集（class/datatype/nodeKind/minCount/maxCount/minLength/maxLength/pattern/in/hasValue/node/closed）；severity/message；`ValidationReport`（conforms 仅 Violation 判不合规）；`sh:pattern` 内置小正则子集；reasoner 4→13 测，全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：SHACL 约束组件扩展——逻辑形状 `sh:and`/`sh:or`/`sh:not`（节点/属性形状通用，`conforms_to` 本地收集判定、depth 递归护栏）；属性形状参数 `sh:qualifiedValueShape` + `sh:qualifiedMinCount`/`sh:qualifiedMaxCount`/`sh:qualifiedValueShapesDisjoint`（同 path 多属性形状按引用取参、sibling 互斥计数）；`sh:closed` 合并 `sh:ignoredProperties` 白名单；reasoner 13→20 测（SHACL 9→16），全量测试 223 通过（server 21 测需沙箱外运行，沙箱内端口绑定被拒）。 |
| 2026-08-07 | Codex | L6：SHACL 约束组件扩展（二）——数值范围 `sh:minInclusive`/`sh:maxInclusive`/`sh:minExclusive`/`sh:maxExclusive`（整数/浮点/字符串比较）；属性对 `sh:equals`/`sh:disjoint`/`sh:lessThan`/`sh:lessThanOrEquals`（同 focus 双路径值集合比较）；`sh:pattern` 支持 `sh:flags`（`i` 大小写不敏感，其余忽略）；reasoner 20→25 测（SHACL 16→21），全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：OWL 2 RL 规则扩展——`cax-sco`（subClassOf 应用：x type C ∧ C subClassOf D → x type D）、`prp-symp`（对称属性）、`prp-trp`（传递属性，链式闭包）、`prp-inv2`（inverseOf 反向应用）；修正 Rule 枚举标注（rdfs5/7 对照）；reasoner 25→30 测（forward-chain 4→9），全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：推理超时护栏（P6-03）——`ReasoningTask.max_elapsed_ms` 墙钟预算（None 不限）+ 每次迭代前检查、`ReasoningReport.timed_out` 早停标记；reasoner 30→31 测，全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：OWL 2 RL 限定词规则——`cls-svf1`（someValuesFrom 正向：x type (p some C) ∧ x p y → y type C）、`cls-svf2`（someValuesFrom 反向定型）、`cls-avf`（allValuesFrom）；支持 bnode 限定词节点；reasoner 31→34 测（forward-chain 9→12），全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：OWL 2 RL 类表达式与等价——`cls-int1`/`cls-int2`（intersectionOf 正向定型 + 反向全成员定型）、`cls-uni`（unionOf 成员→并集定型）、`eq-sym`/`eq-trans`（owl:sameAs 对称/传递闭包）；RDF 列表成员遍历；reasoner 34→37 测（forward-chain 12→15），全量测试待本波提交前验证。 |
| 2026-08-07 | Codex | L6：OWL 2 RL 键与一致性规则——`prp-key`（owl:hasKey 列表键全部共享值 → owl:sameAs，支持多键/列表遍历）、一致性 ⊥ 检测（`cax-dw` 互斥类、`cls-nothing1`/2、`eq-diff1`/2；`ReasoningReport.inconsistent` 标记）；reasoner 37→41 测（forward-chain 15→19），全量测试 244 通过。 |
| 2026-08-07 | Codex | L6：Claude Code 审查修复——F1 prp-key 改值→成员桶索引（消除 O(m^2·k·n^2) 对扫）；F2 eq-sym/eq-trans/eq-diff 改 bnode 感知（NodeId 索引）；F3 一致性规则并入同迭代 frontier（max_iterations=1 也能检出链式 ⊥）；F4 补 6 测（一致输入负断言/单键 hasKey/字面量键值/反向 differentFrom/传递链到 Nothing/环列表）；reasoner 41→47 测（forward-chain 19→25），全量测试 250 通过。 |
| 2026-08-07 | Codex | L6：OWL 2 RL 等价与基数规则——`prp-fp`/`prp-ifp`（功能/逆功能属性 → 值/主词 sameAs）、`eq-rep-s/p/o`（sameAs 主/谓/宾替换，HashMap 索引）、`cls-maxc2`（maxCardinality 1 基数键）、`cls-com`（complementOf 一致性 ⊥）；reasoner 47→52 测（forward-chain 25→30），全量测试 255 通过。 |
| 2026-08-07 | Codex | 计划更新：执行顺序改为自底向上逐层推进（L0→L8，先底层后顶层应用）；§2 焦点表按层重排（P0=L0–L3 底层收尾、P1=L4 多进程 Raft、P2=L5 安全隔离、P3=L6 推理应用化、P4=L7 运维发布）；§8 新增分层后续执行队列，当前光标为 L0–L3 底层收尾。 |
| 2026-08-07 | Codex | L0/L1 底层契约收尾（自底向上队列首项）——首个实质 RFC [RFC-0001](../rfc/0001-canonical-encoding-and-disk-layout.md)（确定性标识/规范化编码/六置换键/RocksDB 磁盘布局，P0-04 试用 + P1-04 定稿）；[L2-storage-contracts.md](./L2-storage-contracts.md)（P1-02 并发字典契约：线程安全/单调分配/epoch 不可变/批写原子；P1-03 存储接口版本冻结 0.1.0 + 变更流程）。 |

---

## 8. 近期行动队列（可勾选）

### 后续执行队列（自底向上，逐层到顶层应用）

原则：先底层逐层到最顶层应用——优先完成当前最低未完成层，再推进上一层；避免跳层开发。R1 退出标准收尾（核心 SLO 基线、恢复/回滚演练、全表勾选）随各层推进同步完成。

> 当前光标：**L0–L3 底层收尾（本队列首项）**

- [x] **L0/L1 底层契约**：P1-02 并发字典契约、P1-03 存储接口版本冻结、P1-04 独立编码 RFC + 磁盘布局（2026-08-07）
- [ ] **L2 存储内核**：P2-02 真 MVCC 版本链 → P2-01 纯 CF 索引扫描 → P2-04 命名图六置换/Async → P2-05 fsync/备份演练
- [ ] **L3 查询引擎**：P3-01 高级 Update（LOAD/CLEAR/WITH）→ P3-02 代价模型/统计 → P3-03 HTTP Explain API → P3-04 异步抢占 → P3-05 W3C 欠账逐项提 PASS
- [ ] **L4 集群**：P4-02 多进程 Raft M1（单节点 openraft 适配）→ M2（多进程 HTTP RPC + RocksDB raft CF）→ M3（默认运行时切换 + CI 三进程 smoke）；P4-01 多进程 RPC、P4-03 跨节点数据搬迁、P4-04 真实网络分区
- [ ] **L5 接入与安全**：P5-03 强制分库/行级租户隔离 → P5-02 OIDC/JWT → P5-05 Tracing 全链路 → P5-01 gRPC
- [ ] **L6 推理与验证**：P6-01 规则扩展收尾（cls-hv1/2、prp-irp、cax-adc/eq-diff2/3 AllDifferent、prp-chain）→ P6-03 接入 server 查询/推理管线 → P6-02 SHACL 补全 + W3C SHACL 套件
- [ ] **L7 平台工程**：P7-01/04 在线重平衡与灾备演练、运维手册与证据包 → P7-02 阈值断言/趋势记录 → P7-03 发布/回滚手册
- [ ] **L8 AI-Native**：R4 启动时立项（P8-01/02/03）

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
- [x] 本波次提交已推送 `origin/main`（直推模式，无 PR）
- [x] 管理面安全加固（TLS 终止方案落地：rustls 进程内终止 + R2 门禁；OIDC 留 R2+ 后续轨）

### 未完成项待办清单（2026-08-06 整理）

**规划与设计（草案 → 定稿）**

- [ ] 评审并签批 PLAN-0001，解除 P0-01（已批准范围基线）阻塞（`docs/PROGRESS.md:85`）
- [ ] 架构规范定稿：`docs/Ontolith_Software_Architecture_Specification.md`（1.2.0-draft）
- [ ] 架构规范定稿：`docs/Ontolith Software Architecture Specification  Volume 04.md`（1.0.0-draft）
- [ ] 架构规范定稿：`docs/SAS-0401 — Knowledge Object Model.md`（1.0.0-draft）
- [ ] 架构手册目录（1.0 Draft）按“Specification Before Implementation”补齐对应章节
- [x] ADR-0003 由 Proposed 转 Accepted，并回填 Phase/WBS 关联（2026-08-06）
- [ ] 首个实质 RFC 试用，完成 P0-04（`docs/PROGRESS.md:88`）
- [ ] PLAN §10 “设计包（接口、约束、ADR 关联）”纳入台账跟踪与验收

**管理面安全（P0，进行中）**

- [x] TLS 终止方案落地（rustls 进程内终止 + `ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY` + `/admin/config` bind 姿态证据 + 自签证书脚本）
- [ ] 或 OIDC 校验链路实现（token 验证 + claim 映射，落在 `crates/ontolith-security` 抽象内；TLS 已落地，本项为 R2+ 后续轨，不阻塞 P0）
- [x] 非 loopback 暴露场景 TLS 强制门禁（R2 判据，ADR-0003 路线：非 loopback bind 无 TLS 拒绝启动）

**SLO 与性能基线（P1，进行中）**

- [x] 天/周窗口 SLO 自动化（systemd timer 采集 5min + 每日 24h / 每周 168h 窗口评估）
- [x] 告警策略（连续失败次数 / 延迟异常突增，`alerts.jsonl` + 退出码门禁）
- [x] `benchmarks/` 性能基线用例（P7-02）：`storage_bench` 微基准 + `benchmarks/README.md` + CI `bench` 作业
- [ ] 核心 SLO 基线达标（R1 检查项，`docs/PROGRESS.md:213`）

**R1 退出标准剩余项**

- [x] 多节点数据面设计定稿：ADR-0004（openraft behind traits + 树内 HTTP RPC + RocksDB raft CF；M1–M3 里程碑；2026-08-06）
- [ ] 多节点数据面实施（M1 单节点 openraft 适配 → M2 多进程 HTTP RPC + RocksDB raft CF → M3 默认运行时切换 + CI 三进程 smoke）
- [x] 完整 W3C 套件接入（vendored `w3c/rdf-tests` sparql11 941 文件/28 feature + manifest 驱动 `w3c11_suite` runner + `w3c11_profile.tsv` 492 条基线：127 PASS / 365 FAIL 欠账 profile 化防回归；2026-08-06）
- [x] 完整聚合（GROUP BY/HAVING + COUNT(DISTINCT)/SUM/AVG/MIN/MAX + 子查询聚合，W3C must-pass 27/27）
- [x] SPARQL Update 基线（INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE，W3C must-pass 30/30、skip=0）
- [ ] 在线重平衡与灾备演练手册及证据（P7-01 / P7-04）
- [ ] 发布流水线与回滚演练通过（`docs/PROGRESS.md:215`）
- [ ] R1 退出标准全表勾选（`docs/PROGRESS.md:375`）

### R1 关键路径（按依赖序）

1. [~] Phase 0 签批与模板（模板齐；签批未做）
2. [~] Phase 1 KO 模型 + 存储契约文档
3. [~] Phase 2 RocksDB + 多索引 + 事务文档
4. [~] Phase 3 真 SPARQL MVP（含完整聚合）+ Explain/超时 + R1 烟雾
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
- [L2 存储契约（并发字典/接口冻结）](./L2-storage-contracts.md)
- [L3 parser/query 功能说明](./L3-ontolith-parser-query.md)
- [L5 access/security 功能说明](./L5-ontolith-access-security.md)
- [L5 管理平台 SLO 基线](./L5-management-platform-slo.md)
- [L4 cluster/consistency 功能说明](./L4-ontolith-cluster-consistency.md)
- [R1 SPARQL 烟雾符合性](./R1-sparql-smoke-compliance.md)
- [CI workflow](../.github/workflows/ci.yml) · [ci-local](../scripts/ci-local.sh)
- [ADR 模板](../adr/0000-template.md) · [RFC 模板](../rfc/0000-template.md)
- [RFC-0001 确定性标识与规范化编码规则](../rfc/0001-canonical-encoding-and-disk-layout.md)
