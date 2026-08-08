# Ontolith 任务进度台账

文档 ID: PROG-0001  
版本: 0.1.36  
状态: Active  
创建: 2026-07-15  
基准: [PLAN-0001](./Ontolith_Development_Plan.zh-CN.md)  
对照代码快照: 2026-07-23（L0–L5 全量实现分批提交完成 + CI/合规烟雾 + W3C 子集门禁 required-lite + strict 观测轨 + 文件审计 + systemd 打包；W3C 子集扩容至 must-pass 24/24；管理平台已纳入主干：`ontolith-management-server` + ACL 分离 + runtime probe + local/CI smoke + SLO 阈值门禁 + 窗口化 SLO 门禁；安全加固 ADR-0003 已起草）；2026-08-06 增量：L0 序列化 Part II、L3 属性路径 `?`（W3C must-pass 25/25）+ RDF 序列化导出、L2 命名图六置换、L4 数据面同步、L5 审计哈希链、L6 前向链推理、L7 存储微基准与 CI bench/license 作业、**完整聚合（GROUP BY/HAVING + COUNT(DISTINCT)/SUM/AVG/MIN/MAX + 子查询聚合，W3C must-pass 27/27）**、**SPARQL Update（INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE，W3C must-pass 30/30、skip=0）**、**天/周窗口 SLO 自动化（systemd timer 采集/日周评估 + 告警策略：成功率/连续失败/P95/尖峰）**、**管理面 TLS 终止（rustls 进程内终止 + `ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY` + `/admin/config` TLS 姿态证据 + 自签证书脚本；ADR-0003 转 Accepted）**、**R2 非 loopback TLS 强制门禁（非 loopback bind 无 TLS 拒绝启动）**、**完整 W3C 套件接入（vendored `w3c/rdf-tests` sparql11 941 文件/28 feature + manifest 驱动 runner：QueryEvaluation/UpdateEvaluation/PositiveSyntax/NegativeSyntax + SRX/SRJ/TSV/CSV/Turtle/ASK 结果比对 + profile 锁定 492 条基线，127 PASS/365 FAIL 作为合规欠账）**、**Turtle 数字字面量词法修复（`.` 不再作为分隔符，完整 INTEGER/DECIMAL/DOUBLE 文法 + `.5` 前导点小数，parser 16→17 测）**；**第七波：SPARQL Update 图管理全量（ADD/COPY/MOVE/CREATE + SILENT + DEFAULT/GRAPH 目标）与更新语义收尾（USING/USING NAMED 数据集子句、INSERT/DELETE DATA 命名图与 bnode、DELETE 模板 bnode 拒绝、请求多操作 `;` 分隔与尾随校验、DATA bnode 标签 request 级作用域、请求内更新顺序可见性——命名图 pending 读取），W3C 339→434 PASS（+95，剩余 58 项 semantic/parse 欠账）**；**第八波：空白节点属性列表（路径谓词）/RDF 集合 + 子查询 SELECT 投影表达式与 §18.2.2 变量作用域 + 模板 bnode 按解实例化 + NodeId 字典解码 + BGP Node≡Blank 等价 + harness 图比对标签位置化归一，W3C 434→444 PASS（+10）**；**第九波：聚合补全（表达式聚合参数 AVG(IF/COALESCE)、嵌套聚合投影/HAVING 多条件、GROUP_CONCAT DISTINCT/纯字符串、AVG 空组 0/错误传播、decimal SUM 精确累加）+ rs:ResultSet 结果比对 + graphData 直接 IRI/无 label 回退，W3C 444→458 PASS（+14，剩余 34 项欠账）**；**第十波：syntax-query 10 项全收尾（负向 8：`SELECT *`+GROUP BY/尾随内容/组中段子查询/数字前缀名/非法 `\` 转义/部分代理对 `\uD800`；正向 2：`::` 前缀与 `_12.3_` 本地名、空前缀函数）+ FROM/FROM NAMED 数据集子句（§18.2.1）+ 组内 FILTER 延后应用 + USING NAMED 不回退全命名图 + STR 解码 Node/Blank + harness RDF/XML 读取与裸文件名 base，W3C 458→492 PASS（+34，fail=0、drift=0，492/492 全绿）**；**L4 M1：多进程 Raft 数据面落地第一步（ADR-0004）——openraft 0.9.25（Tier A 锁定，`raft-backend` feature 默认开 + 无 feature 回退构建）+ v1 `RaftStorage` 内存存储（日志/状态机/快照）+ `Adaptor` 适配 + 内存 `RaftNetworkFactory` 传输（`RaftRegistry` 进程内路由）+ `RaftClusterRuntime` 集群 trait 适配（选主/epoch/复制日志/commit 由 openraft 背书，分片路由等控制面工具委托模拟器）；cluster 17→21 测（单节点引导自选主、client_write→commit、trait 适配往返、双节点内存传输选举+复制）**；**L4 M2：多进程 Raft 数据面第二步（ADR-0004 决策 2/3）——openraft `serde` feature + `serde_json` 树内 HTTP/1.1 RPC（`/internal/raft/{vote,append-entries,install-snapshot}`，共享 secret `Authorization: Bearer` 认证，沿用 L5 同风格最小 HTTP 栈，未引入 axum/reqwest）+ RocksDB 独立 `raft` CF（`RocksDbStorageEngine::raft_cf_*` 字节级原语 + `RocksRaftStorage` v1 `RaftStorage`：日志/vote/committed/last_applied/last_purged/membership/应用态/快照字节，经 `Adaptor` 接入）+ `RaftClusterRuntime` 配置化选择（`http_listen_addr`/`raft_secret`/`raft_storage_path`，内存传输与内存存储保留为测试回退）+ snapshot build/install 全链路（`RocksSnapshotBuilder` 序列化应用态、`install_snapshot` 原子替换 `applied/*` + 快照引用）；cluster 21→26 测（RocksDB 日志/快照往返 + HTTP 共享 secret 拒绝 + HTTP install-snapshot RPC 往返 + 双节点 HTTP+RocksDB 选举/复制/落盘）**；**L4 M3：多进程 Raft 数据面落地第三步（ADR-0004 决策 5）——默认运行时切换：`AppState.cluster` 改 `Arc<dyn ClusterRuntime>`，管理二进制 `ONTOLITH_CLUSTER_MODE` 默认 `raft`（内存模拟器降级为测试/CI harness，`memory` 可显式选择），raft 模式经 `ONTOLITH_RAFT_NODE_ID`/`ONTOLITH_RAFT_LISTEN`/`ONTOLITH_RAFT_SECRET`/`ONTOLITH_RAFT_MEMBERS`/`ONTOLITH_RAFT_STORAGE_PATH` 配置固定成员引导（多节点同成员 `initialize` 容忍 `NotAllowed` 安全忽略）；`Replicator::replicate_to_followers` 真实语义（leader replication metrics 水印增量）+ `applied_index` 返回 follower acked index + `/admin/data/replicate?append=1` 驱动 raft 写入；cluster 26→27 测（三节点 HTTP+RocksDB 多数派提交、失一 follower 后仍可提交）+ CI 三进程 multi-node raft smoke（3 进程引导、leader 观测、append→全节点 commit 推进、杀 follower 后多数派继续提交）**；**L4 P4-01：多进程元数据服务与主从选举收尾——`LogPayload` 增 `RegisterNode`/`Heartbeat`/`SetNodeStatus` 元数据变异变体（`ClusterNode`/`NodeRole`/`NodeStatus`/`RegionId`/`ClusterNodeId` 增 serde），`RaftClusterRuntime` 增复制式节点注册表（`nodes` 注册表 + `applied_watermark` 增量折叠 applied 日志，`sync_applied` 幂等收敛，RocksDB 重启后从持久化 applied 条目重建）；`register_node`/`heartbeat`/`set_node_status` 经 raft 提交：leader 本地 `client_write`，follower 经新增 `/internal/raft/apply` 元数据 RPC 转发至 leader（409 携带 leader 提示、最多重试 3 次）；`membership()`/`status()` 改读复制式注册表（role 按当前 leader 刷新，bootstrap 种子固定成员）；cluster 27→28 测（三节点 HTTP+RocksDB 下 follower 发起 register/heartbeat/status 全节点收敛、leader 直发路径、node_count 5）**；**L4 P4-03/P4-04：跨节点数据搬迁 + 真实网络分区——`DataPlaneSync for RaftClusterRuntime` 升级为真实实现：新增 `DataPlaneSnapshotIo` trait（`export_snapshot`/`import_snapshot`，进程接入 L2 存储）与 `/internal/raft/transfer-snapshot` RPC（`TransferSnapshotRequest{shard_id,slots,snapshot_id,bytes}`，Bearer 认证，目标无 hook 返回 503）；`complete_transfer` 经调用节点本地 IO hook 导出快照字节、校验目标非分区且在成员表、POST 至目标节点 HTTP 端点导入，200 返回 `SyncReceipt`（`transferred_entries=snapshot_id`、`completed_at_epoch=current_epoch`），无 hook 保留模拟回执回退；`FaultInjector` 升级为真实对称网络分区：`HttpRaftClient::post` 在 target/self 处于 `partition` 集合时返回 `RPCError::Network`（raft 层对称丢弃），`metadata_mutation` 拒绝被隔离节点自身操作与转发到被隔离 leader，`complete_transfer` 拒绝迁移到被分区目标；cluster 28→30 测（三节点 HTTP+RocksDB：快照字节经 HTTP 迁移到目标并记录导入调用、无 hook 回退回执不触网、隔离 leader+一 follower 后元数据转发确定性失败、heal 后重新选主与转发恢复）**；**L5 P5-03：强制分库/行级租户隔离——`TenantMode`（`ONTOLITH_TENANT_MODE=enforced`）+ `TenantNamespace`（`urn:tenant:<t>` 命名空间，`require_owned` 拒绝越权引用）；`QueryRequest.tenant_scope` 下沉到查询执行器：`TenantScopedRead`/`TenantScopedWrite` 视图把默认图重指向租户图、命名图可见性限制在租户命名空间，`FROM`/`GRAPH`/`USING`/图管理目标等显式图引用越权即 403，更新默认图写自动盖章进租户图；server 写路径强制盖章（`?graph=` 越权 403、TriG 命名图越权 403），读路径注入 tenant scope，`/health`/`/admin/config` 暴露 `tenant_mode`；security 9→12 测、query 83→86 测、server 22→24 测（acme/other 互不可见、越权引用 403、默认图写盖章进 `urn:tenant:acme`）**；**L5 P5-02：OIDC/JWT 鉴权基线——树内 HS256 JWT 验证（RFC 7519 子集：base64url RFC 4648 §5、SHA-256 FIPS 180-4、HMAC-SHA256 RFC 2104、常量时间签名比对，FIPS/RFC 4231 向量背书）+ `Authorization: Bearer` 接入（`ONTOLITH_JWT_SECRET`/`ONTOLITH_JWT_ISSUER`/`ONTOLITH_JWT_AUDIENCE`，`exp`/`iss`/`aud` 校验，JWT tenant claim 优先于传输头，`/health` 暴露 `jwt` 姿态）；security 12→18 测（jwt sign/verify 往返、篡改/过期/iss/aud 拒绝、bearer 鉴权）、server 24→26 测（Bearer 认证、伪造/过期 401、JWT 租户优先盖章）**；**L5 P5-05：Tracing 全链路——`ontolith-observability` 新增追踪域模型（`TraceId`/`SpanId`/`SpanEvent`/`SpanStatus`/`TraceContext`）+ 有界内存 span 存储（`InMemoryTraceStore`，1024 cap 逐旧）+ 确定性 id 生成（128-bit trace / 64-bit span，hex）+ W3C `traceparent` 解析/生成（`00-<32hex>-<16hex>-01`，上游 trace 延续）+ 线程本地 RAII `TraceScope`（子 span 无需改 17 处 handler 签名）；server 网关全链路埋点：`http.request` 根 span + `http.auth`/`sparql.execute`/`data.ingest` 子 span（父链 + 状态 + method/path/status/latency/tenant 属性），响应回带 `Traceparent` 头供下游延续，`/health`/`/admin/config` 暴露 `tracing` 姿态，管理面新增 `GET /admin/traces`（按 trace 分组、新→旧、limit）；observability 6→11 测（id 唯一性/hex、traceparent 往返与畸形拒绝、作用域嵌套恢复、cap 淘汰、JSON 分组）、server 26→29 测（全链路 span 父链与回带、失败请求 auth/root error 状态、`/admin/traces` 列表）**；**P5-01 gRPC 网关接入（tonic 0.12 + prost 0.13 + `protoc-bin-vendored` 3，`grpc-backend` feature 默认开 + `--no-default-features` 回退构建）：`proto/ontolith/v1/sparql.proto` `SparqlService{Query,Health}`；`SparqlGateway` 复用 HTTP 网关共享执行路径（`execute_sparql`/`explain_sparql`/`explain_json`/`sparql_results_json` 提为 `pub(crate)`）+ 同构鉴权（metadata `x-ontolith-tenant`/`x-ontolith-user`/`x-api-key`/`authorization`，enforced 401/跨租户 403）+ W3C `traceparent` 延续与回带 + 根/子 span 埋点；`serve_grpc` 独立 tokio multi_thread runtime 线程；`ontolith-server` bin bootstrap 由 metrics 演示升级为真实双网关（HTTP `ONTOLITH_BIND` + gRPC `ONTOLITH_GRPC_BIND` 默认 `127.0.0.1:50051`，共享 env 契约 `build_gateway_app_state_from_env`）；server 29→33 测（roundtrip insert/select、enforced 401、跨租户 403、health）**；**L6 P6-02：SHACL 核心组件补全（语言标签管道重构 + `sh:languageIn`/`sh:uniqueLang`/`sh:xone`，SHACL 21→30 测、reasoner 59→68 测）**；**L6 P6-02：W3C SHACL 核心套件接入（vendored `w3c/data-shapes` core 121 个 `sht:Validate`/98 可运行 + `shacl_suite` manifest runner + `w3c-shacl_profile.tsv` 84 PASS/14 FAIL 基线锁定：60→84 PASS；配套引擎补全：节点形状属性对、`sh:deactivated`、自定义 severity、W3C conforms 语义、`str()` 长度/pattern 语义 + `{n,m}` 量词、datatype 词法合法性、languageIn 语言范围、uniqueLang 单条无 value、`sh:node` 单条结果、closed 不豁免 rdf:type、独立/嵌套属性形状、dateTime 比较；SHACL 30→35 测、reasoner 68→73 测）**；**L6 P6-02 收尾：SHACL 属性路径表达式全量（`sh:path` 支持 `sh:inversePath`/`sh:alternativePath`/RDF 列表 sequence/`sh:zeroOrMorePath`/`sh:oneOrMorePath`/`sh:zeroOrOnePath`，SPARQL 集合语义去重 + 闭包环路护栏 + 结果路径 canonical 序列化，`PropertyPath` 域类型），W3C SHACL 核心套件 84→97 PASS（12 项 path/* 属性路径 + shacl-shacl 元校验转绿，唯一缺口 uniqueLang-002 `"1"^^xsd:boolean` 词法差异 profile 锁定）；SHACL 35→42 测、reasoner 73→80 测；w3c11 492/492 保持全绿**；全量测试已验证（w3c11 492/492、shacl profile drift=0）**；**L7 平台工程（P7-01/04 在线重平衡与灾备演练脚本 + P7-02 阈值断言/趋势记录 + P7-03 发布/回滚手册）**：`scripts/drill-rebalance-dr.sh` 真实 3 进程 raft 集群走完 选主→在线重平衡（`initial_slot_bias` 偏斜 + `shard_map_epoch` 证据）→复制收敛→杀 follower（多数派提交）→重启追赶→杀 leader（自动 failover）→重启追赶（`=== DRILL PASS ===`，约 6 秒）；`scripts/check-bench-thresholds.sh` 按 case 阈值断言 + JSONL 趋势记录（CI `bench` 作业已切换硬门禁）；cluster 30→31 测（`rebalance_moves_slots_after_initial_bias`）；**R1 收尾：幂等写入验证（storage 46→51 测：内存 4 测 + RocksDB 1 测，Put 集合语义去重/重放去重、重复 commit 拒绝、Delete 不存在 no-op、Quad 去重+删除、reopen 持久） + 核心 SLO 基线达标（实测 20 样本 success 100%、p95=0ms、max=3ms，阈值 250ms，见 L5-management-platform-slo.md §5）+ R1 退出标准全表勾选（SPARQL 查询基线/单区域集群核心/标准符合性门禁/SLO 基线转已完成，R1 判定上修至 ~85–88%）；**P7-03 实际发布回滚演练（2026-08-08：`scripts/release-rollback-drill.sh` staging DRILL PASS——V_new=9fac343→V_prev=ec1d539 代码级回滚→mv/rm/cp 数据级回滚→恢复 V_new；`/health triples` 权威计数 1→1→0→1、二进制指纹 `shard_map_epoch` 2→0→2；证据包 transcript）**；**R1 正式验收包（2026-08-08：[R1-acceptance-package.md](./R1-acceptance-package.md) ACC-R1-0001 + `scripts/acceptance-r1.sh`，G1–G5 全 PASS：fmt/clippy 零告警、workspace 20 个 test binary 全 ok 共 400 测、w3c11 492/492 + shacl 97/98 drift=0、内存 INSERT/SELECT 闭环、RocksDB reopen 持久闭环；验收中发现并修复 HTTP 结果 JSON 将 IRI 主语渲染为 bnode 的缺陷（`bound_value_json` 未走字典解码，server 43→44 测））**

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
| Phase 2 持久化与事务内核 | 部分完成 | ~95% | 内存+磁盘 MVCC 版本链（跨重启持久）+ RocksDB 耐久 + 纯 CF 索引扫描（SPO/POS/OSP + 命名图 GSPO/GPOS/GOSP） |
| Phase 3 查询引擎 | 部分完成 | ~96% | Turtle/TriG + SPARQL 核心代数/优化/绑定 + 完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）+ SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）+ 子查询基线 + 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ W3C 子集门禁（required-lite，must-pass 30/30）+ strict 观测轨 + **完整 W3C 套件 manifest 基线（492 条，492 PASS/0 FAIL，fail=0、drift=0）** |
| Phase 4 集群与一致性 MVP | 部分完成 | ~82% | +session 粘性/quorum commit/partition/rebalance + L5 /cluster API + 数据面同步接口（快照迁移入队/回执）；无多进程 Raft |
| Phase 5 接入层与安全基线 | 部分完成 | ~90% | HTTP 全路由 + 文件审计（含哈希链）+ cluster 权限 + systemd 打包 + 独立管理服务器（配置/监控/数据管理）+ 管理 ACL + runtime probe；无 TLS/OIDC |
| Phase 6 推理与验证 | 已完成 | ~100% | 前向链推理引擎（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key、prp-spo2 属性链、cls-hv1/2 hasValue、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2/3（AllDifferent）+ prp-irp/cax-adc（bnode 感知 + 同迭代检测），迭代上限 + 墙钟超时护栏）可用；**SHACL 基线校验引擎落地（目标/核心约束组件全齐 + 属性路径表达式全量 + W3C SHACL 核心套件 97/98，reasoner 4→80 测）** |
| Phase 7 企业运维与发布 | 部分完成 | ~70% | GitHub Actions CI + 本地 ci-local + systemd 部署脚本（含 management server）+ 管理面 smoke + 窗口化 SLO 门禁 + 存储微基准（CI bench 作业，**已接阈值断言 + 趋势记录硬门禁**）+ license 审计 CI 作业 + **在线重平衡与灾备演练脚本（P7-01/04，真实 3 进程 raft，DRILL PASS）** + **发布/回滚手册（P7-03）** |
| Phase 8 AI-Native 扩展 | 未开始 | 0% | — |
| **分层内核 L0–L3** | **部分完成** | **~92–96%** | 语义+存储+查询主路径可用，完整聚合/Update/子查询/属性路径最小集（含 `?`）已纳入回归保护 |
| **相对 R1 退出标准** | **进行中** | **~95%** | 内核+HTTP+集群数据面（多进程 raft M1–M3 + P4-01–P4-04）+ CI/烟雾合规 + W3C 子集 required-lite（30/30）+ 完整 W3C 套件 492/492 全绿 + 核心 SLO 实测达标（success 100%、p95=0ms）+ 恢复/灾备演练 DRILL PASS + 实际发布回滚演练 DRILL PASS + **R1 正式验收包全 PASS**；剩余：OIDC 完整链路（R2+） |
| **相对 R1–R4 全计划** | **进行中** | **~16%** | — |

### 架构分层完成度（实现视图）

| 层 | 完成度 | 状态 |
|----|--------|------|
| L0 core | ~90% | KO/Canonical/Error/ConsistencyLevel/序列化 Part II（20 测） |
| L1 rdf | ~80% | Triple/Quad/Dataset |
| L2 storage/txn | ~97% | 内存 MVCC 版本链（版本快照/剪枝/WAL 重放重建）+ RocksDB 磁盘 MVCC 版本链（versions CF 跨重启持久）+ 纯 CF 索引扫描（SPO/POS/OSP + 命名图 GSPO/GPOS/GOSP，无内存索引重建） |
| L3 parser/query | ~96% | 完整核心，非仅 MVP；完整聚合 + SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）+子查询（含聚合）+属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ RDF 序列化导出；W3C 子集 required-lite（30/30）+ strict 观测双轨 + 完整 W3C 套件 manifest 基线 |
| L4 cluster | ~85% | +session/partition/rebalance/commit + HTTP /cluster + 数据面同步（快照迁移/回执）+ 多进程 raft M1–M3 + P4-01–P4-04 + 在线重平衡（slot bias + shard_map_epoch）；31 测 |
| L5 server/security/obs | ~90% | 双后端、文件审计（哈希链）、Results JSON、ingest、增强指标、部署脚本、管理面二进制与管理 API + ACL + runtime probe |
| L6 reasoner | ~72% | 前向链推理引擎（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key（值→成员桶索引）、prp-spo2 属性链、cls-hv1/2 hasValue、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2/3 + prp-irp/cax-adc（bnode 感知 + 同迭代检测）、迭代上限 + 墙钟超时）+ SHACL 基线校验（目标四选 + 隐式类目标；核心约束组件全齐 + 属性路径表达式全量 inversePath/alternativePath/sequence/zeroOrMore/oneOrMore/zeroOrOne；severity/message；ValidationReport；W3C SHACL 核心套件 97/98）；80 测 |
| L7 平台工程 | ~70% | CI workflow + ci-local + compliance crate + systemd 安装脚本 + 管理面 smoke + 窗口化 SLO 校验 + 存储微基准（阈值断言 + 趋势记录）+ license 审计作业 + 在线重平衡/灾备演练脚本 + 发布回滚手册 |
| L8 AI-Native | 0% | — |

### 当前焦点

| 优先级 | 焦点 | 负责人 | 目标日期 |
|--------|------|--------|----------|
| P0 | L0–L3 底层收尾（编码/字典契约✅、存储 MVCC 版本链✅；查询代价模型与高级 Update、W3C 欠账提 PASS） | TBD | 进行中 |
| P1 | L4 集群多进程 Raft 实施（P4-02 **M1–M3 完成**：openraft 适配 → 多进程 HTTP RPC + RocksDB raft CF + snapshot install → 默认运行时切换 + CI 三进程 smoke） | TBD | TBD |
| P2 | L5 应用层安全与隔离（P5-01 gRPC 网关 **完成**、P5-02 OIDC/JWT **完成**、**OIDC 完整链路 R2+ 完成**、P5-03 强制租户隔离 **完成**、P5-05 Tracing 全链路 **完成**） | 进行中 | 100% |
| P3 | L6 推理应用化（P6-01 规则扩展 **完成** → P6-03 接入 server 查询/推理管线 **完成** → P6-02 SHACL 核心组件补全 + W3C SHACL 套件接入 **完成** → 属性路径表达式与 shacl-shacl 元校验 **完成**（97/98 基线 profile 锁定）） | 进行中 | 100% |
| P4 | L7 运维演练与发布（P7-01/04 演练与运维手册 **完成**、P7-02 阈值断言/趋势记录 **完成**、P7-03 发布/回滚手册 **完成**） | 进行中 | 70% |

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
| P2-01 | RocksDB 适配（抽象层下） | 部分完成 | 90% | `RocksDbStorageEngine` + CF + ADR-0001；纯 CF 索引扫描（SPO/POS/OSP 索引 CF + 前缀扫描读路径，删除内存索引重建，旧库打开自动回填索引 CF，storage 38→37 测） | 运维参数调优（bloom filter/压缩/缓存） |
| P2-02 | WAL / 快照恢复 / MVCC 基线 | 已完成 | 100% | 内存+Rocks WAL CF、reopen 恢复、snapshot+consistency；内存 MVCC 版本链（`versions` 链 + pin/剪枝 + WAL 重放重建）；repo 层按版本读取；RocksDB 磁盘 MVCC 版本链（`versions`/`versions_quads` CF，键 = BE version ‖ 物理键，`meta.next_version` 持久化，提交铸全量快照 + 剪枝/pin，旧库打开自动回填 v1，storage 40 测） | — |
| P2-03 | 三元组/四元组物理编码 | 部分完成 | 90% | codec + 六置换键 + CF 落盘 | 列族级索引键直接扫描 |
| P2-04 | 索引基线 SPO/POS/OSP | 已完成 | 100% | 默认图 SPO/POS/OSP CF 前缀扫描 + 命名图 GSPO/GPOS/GOSP 索引 CF（graph‖S/P/O 置换，绑定位置选择最选择性前缀，storage 43 测）；旧库打开自动回填索引 CF | Async 维护（预留：正确性优先，索引维护保持同步；后续可用“水位+主 CF 回退”方案） |
| P2-05 | 可恢复耐久写入路径 | 已完成 | 100% | `RocksDbOptions`（`sync_writes` 默认 true，commit/delete/字典/WAL 追加走 `WriteOptions::set_sync` fsync）+ `open_with_options`；BackupEngine `create_backup`（提交锁串行化 + flush 后快照）/`restore_backup` 演练（MVCC 版本随备份恢复）；storage 43→46 测 | 备份调度/保留策略接入管理面（运维轨） |
| P2-06 | 事务行为规范文档 | 部分完成 | 95% | [L2 文档 v3](./L2-ontolith-storage-transaction-kernel.md) | 随真 MVCC 修订 |

**阶段退出条件：** 耐久写入可恢复；至少 SPO/POS/OSP；事务文档发布。

---

### Phase 3 — 查询引擎 MVP

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P3-01 | SPARQL 解析到执行主链路 | 已完成 | 100% | SELECT/ASK/CONSTRUCT + JOIN/OPT/UNION/FILTER/BIND/VALUES + 完整聚合 + SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE、**CLEAR/DROP 图作用域、WITH 图作用域、LOAD 本地图复制**）+ 子查询基线 + 属性路径最小集 + RDF 序列化导出；修复空更新误报 pending txn 的 bug；query 46→55 测，W3C 基线 127→151 PASS（24 项 FAIL→PASS） | — |
| P3-02 | 规则优化基线 | 已完成 | 100% | BGP 重排、Identity 消除、Filter 下推、POS/OSP 选路 + **代价模型/统计**（`QueryStatistics` 契约 + `EngineQueryStatistics` 引擎增量统计 + `CostBasedOptimizer` 贪心 join 序与绑定传播，语义保持；`update_pipeline`/`cost_pipeline` 接入）；query 55→58 测，W3C 套件基线无漂移 | — |
| P3-03 | Explain 输出 | 已完成 | 100% | logical/physical/algebra + optimize 步骤 + **HTTP Explain API 成本信息**（`/explain` JSON 输出 `estimated_rows` 与 `pattern_costs`，代价优化器填充）；query 58→59 测，server 19→20 测 | — |
| P3-04 | 超时与取消 API | 已完成 | 100% | timeout_ms + Arc\<AtomicBool\> cancel + **异步抢占 token**（`PreemptionToken`：deadline + cancel 标志 + `reason()` Timeout/Cancelled + 跨线程 `preempt()`）；执行器轮询细化到 BGP 候选/join 行/FILTER/EXTEND/VALUES 行，Update 抢占返回标志且不落写；query 59→63 测 | — |
| P3-05 | MVP 标准符合性子集 | 已完成 | 100% | 引擎单测 + [ontolith-compliance](../crates/ontolith-compliance) 17 烟雾 + W3C 子集运行器（must-pass 30/30，known-gap xfail=0，unsupported skip=0）+ **完整 W3C 套件 manifest 驱动 runner（`w3c11_suite`，492 条基线，492 PASS/0 FAIL，fail=0、drift=0 全绿）** + CI required-lite + strict observer + strict-promotion-readiness 自动信号 + `ci-local.sh` 全链路通过 | 观察主干连续 3 次 CI 全绿后评估 strict required（P3-05 已落地十波：函数/投影/算术/EXISTS/MINUS/聚合/CAST/构造语法 + 完整 datatype/lang 字面量模型 + harness 数值归一/相对 IRI/字符串函数兼容语义 + 哈希/REPLACE/BNODE/REGEX/UUID/相对 IRIREF + 属性路径全量（NPS/零长/字面量零长项）+ 图管理/更新语义 + 第八波 空白节点属性列表/RDF 集合/子查询投影表达式与作用域 + 第九波 聚合补全（表达式聚合参数/嵌套聚合投影/HAVING 多条件/GROUP_CONCAT DISTINCT/AVG 空组与错误传播/decimal 精确累加）+ rs:ResultSet 结果比对 + graphData 直接 IRI 与无 label 回退 + 第十波 syntax-query 收尾/数据集子句/FILTER 延后/Unicode 转义，W3C 192→229→265→284→322→339→434→444→458→492 PASS；欠账清零） |

**阶段退出条件：** MVP profile 查询可跑通；Explain/超时/取消可用。

---

### Phase 4 — 集群与一致性 MVP

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P4-01 | 元数据服务与主从选举 | **完成** | 100% | membership/status + bootstrap + 分区感知选主 + **多进程元数据 RPC**（`/internal/raft/apply`：register/heartbeat/set_node_status 经 raft 提交并在全节点复制注册表收敛，cluster 27→28 测） | 无（P4-01 全项落地） |
| P4-02 | Raft 控制基线 | **完成** | 100% | 任期/日志 + **commit_index 多数派**；[ADR-0004](../adr/0004-multi-process-raft-data-plane.md) **M1 openraft 适配 + M2 多进程 HTTP RPC + RocksDB raft CF + snapshot install + M3 默认运行时切换 + CI 三进程 smoke**（cluster 26→27 测，三节点 HTTP+RocksDB 多数派提交/失一 follower 后仍可提交） | 无（P4-02 全项落地） |
| P4-03 | 单区域分片与复制 | **完成** | 100% | hash slot + lag + **rebalance** + **跨节点数据搬迁**（`DataPlaneSnapshotIo` export/import + `/internal/raft/transfer-snapshot` 真实字节迁移，cluster 28→29 测） | 无（P4-03 全项落地） |
| P4-04 | 故障转移基线 | **完成** | 100% | failover + **partition 注入/愈合** + **真实网络分区**（`HttpRaftClient` 对称丢弃 + `metadata_mutation` 隔离拒绝/愈合恢复，cluster 29→30 测） | 无（P4-04 全项落地） |
| P4-05 | 读一致性级别与 API 说明 | 部分完成 | 95% | Session 粘性 + [L4 文档 v2](./L4-ontolith-cluster-consistency.md) + **/cluster HTTP** | — |

**阶段退出条件：** 单区域复制 + 选主/故障转移可演示。

---

### Phase 5 — 接入层与安全基线

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P5-01 | 网关与服务接入边界 | **完成** | 100% | 全路由 + memory/rocksdb 工厂 + SPARQL Results JSON + 独立 `ontolith-management-server` 管理面 + 健康探测 + **gRPC 网关**（tonic+prost：`SparqlService{Query,Health}` 真实 HTTP/2，metadata 鉴权同构 HTTP（enforced 401/跨租户 403）+ `traceparent` 延续/回带 + 根/子 span + `ONTOLITH_GRPC_BIND`（默认 `127.0.0.1:50051`），`ontolith-server` bin 双网关可执行，server 29→33 测） | 无（P5-01 全项落地） |
| P5-02 | 鉴权 / 授权 | **完成** | 100% | Header/API-Key + Permission + `cluster:admin` + 管理面 read/write ACL + **OIDC-ready JWT**（树内 HS256：`Authorization: Bearer` + `ONTOLITH_JWT_SECRET`/`ONTOLITH_JWT_ISSUER`/`ONTOLITH_JWT_AUDIENCE`，`exp`/`iss`/`aud` 校验、JWT tenant claim 优先，security 12→18 测、server 24→26 测） | 无（远程 JWKS 留 OIDC 后续轨） |
| P5-03 | 租户隔离 | **完成** | 100% | 审计租户过滤 + `tenant_graph` 写入命名图 + **强制分库/行级**（`ONTOLITH_TENANT_MODE=enforced`：`TenantNamespace` + 执行器 `TenantScopedRead/Write`，默认图重指向租户图、越权图引用 403，security 9→12 测、query 83→86 测、server 22→24 测） | 无（P5-03 全项落地） |
| P5-04 | 审计日志 | 部分完成 | 90% | 内存 + `FileAuditLog` JSONL（`ONTOLITH_AUDIT_PATH`）+ 哈希链（`prev`/`hash` + `verify_chain`） | 加密级哈希升级（可选） |
| P5-05 | 指标 / 追踪 / 日志基线 | **完成** | 100% | 延迟/状态码/错误计数 + access log + 管理面监控聚合视图（`/admin/monitoring`）+ runtime probe + **Tracing 全链路**（`traceparent` 延续 + `http.request` 根 span + `http.auth`/`sparql.execute`/`data.ingest` 子 span + `Traceparent` 回带 + `/admin/traces`，observability 6→11 测、server 26→29 测） | 无（P5-05 全项落地） |

**阶段退出条件：** 安全基线挂在真实请求路径；统一遥测可用。

---

### Phase 6 — 推理与验证增强

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P6-01 | OWL 2 RL 核心规则 | 已完成 | 100% | `ForwardChainReasoner`：rdfs5/6/7/8/9 + prp-inv1/2 + prp-symp + prp-trp + prp-fp（功能属性→值 sameAs）+ prp-ifp（逆功能属性→主词 sameAs）+ cax-sco + cls-svf1/2 + cls-avf + cls-hv1/2（owl:hasValue 双向：定型→值三元组 / 值匹配→定型，含字面量值）+ cls-int1/2 + cls-uni + cls-maxc2（maxCardinality 1 → 值 sameAs）+ eq-sym/trans + eq-rep-s/p/o（sameAs 主/谓/宾替换）+ prp-key（owl:hasKey 列表键共享值→sameAs，值→成员桶索引）+ prp-spo2（owl:propertyChainAxiom 属性链，任意长度逐跳 join）+ 一致性 ⊥ 检测（cax-dw/cls-com/cls-nothing1/2/eq-diff1/2/3（AllDifferent members/distinctMembers 同对 sameAs）+ prp-irp（IrreflexiveProperty 自反）+ cax-adc（AllDisjointClasses 双类型），`ReasoningReport.inconsistent` 标记；bnode 感知 + 同迭代 frontier 检测）（含 bnode 限定词/列表表达式）、迭代闭包、`InferenceMode` 开关、38 测 | — |
| P6-02 | SHACL 基线验证 | 已完成 | 100% | `ShaclEngine`（[infrastructure/shacl.rs](../crates/ontolith-reasoner/src/infrastructure/shacl.rs)）：形状解析（节点/属性形状 + 独立属性形状 `sh:path`、`sh:property` 嵌套递归、RDF 列表 `sh:in`/`sh:and`/`sh:or`/`sh:xone`/`sh:ignoredProperties`）、目标选择（targetClass/targetNode/targetSubjectsOf/targetObjectsOf + 形状自身为 rdfs:Class/owl:Class 的隐式类目标）、约束子集（class/datatype(+词法合法性校验)/nodeKind/minCount/maxCount/minLength/maxLength/pattern(+flags)/in/hasValue/languageIn/uniqueLang/node/and/or/xone/not/closed + 数值范围（含 xsd:dateTime 带/不带时区比较）+ 属性对 equals/disjoint/lessThan/lessThanOrEquals——节点/属性形状通用——SHACL 核心约束组件全齐）、属性形状参数（qualifiedValueShape + qualifiedMinCount/qualifiedMaxCount/qualifiedValueShapesDisjoint、ignoredProperties 并入 closed 白名单）、severity/message（自定义 severity IRI 原样保留）、`sh:deactivated true` 跳过形状、`ValidationReport`（conforms = 无任何结果，Warning/Info 也计不合规）；语言标签管道：`term_key`/`focus_as_term`/`literal_string` 语言标签参与 RDF 项相等、`sh:languageIn` 基础语言范围匹配（en 匹配 en-NZ）、`sh:uniqueLang` 每 focus 单条结果无 sh:value；`sh:minLength`/`sh:maxLength`/`sh:pattern` 按 SPARQL `str()` 语义作用于 IRI 全串与字面量词法（bnode 恒失败）；`sh:pattern` 小正则子集（search 语义 + `^`/`$` 锚定 + `*+?` 与 `{n}`/`{n,}`/`{n,m}`）；`sh:node` 单条 NodeConstraintComponent 结果；**W3C SHACL 核心套件接入**（vendored `w3c/data-shapes` core 121 个 sht:Validate/98 可运行 + `shacl_suite` manifest runner + `w3c-shacl_profile.tsv` 84 PASS/14 FAIL 基线锁定：12 项 path/* 属性路径 unsupported + shacl-shacl 元校验 + uniqueLang `"1"^^xsd:boolean` 词法差异）；**属性路径表达式全量**（`PropertyPath` 域类型：`sh:inversePath`/`sh:alternativePath`/RDF 列表 sequence/`sh:zeroOrMorePath`/`sh:oneOrMorePath`/`sh:zeroOrOnePath`，SPARQL 集合语义去重、闭包环路护栏、结果路径 canonical 序列化；12 项 path/* 全转绿）+ **shacl-shacl 元校验通过**（shacl-shacl 以自身为数据/形状全绿）；W3C SHACL 核心套件 84→97 PASS/1 FAIL（唯一缺口 uniqueLang-002 `"1"^^xsd:boolean` 词法差异，profile 锁定）；42 测（reasoner 共 80） | —
| P6-03 | 可配置推理模式与保护 | 已完成 | 100% | `InferenceConfig`（`ONTOLITH_INFERENCE_MODE`=off/forward/hybrid、`ONTOLITH_INFERENCE_MAX_ITERATIONS`=64、`ONTOLITH_INFERENCE_MAX_ELAPSED_MS`）+ HTTP `?inference=` 每请求覆盖（非法 400）+ `execute_sparql_with_inference` 共享路径（HTTP/gRPC，Update/Explain 跳过）+ `ReasoningReadService` overlay（仅叠加增量闭包）+ enforced 租户隔离输入 + reasoning meta（inferred_triples/elapsed_ms/timed_out/inconsistent） | — |

**阶段退出条件：** RL 核心 + SHACL 基线 + 性能护栏可配置。

---

### Phase 7 — 企业运维与发布工程化

| ID | 交付物 | 状态 | 完成度 | 证据 | 下次动作 |
|----|--------|------|--------|------|----------|
| P7-01 | 在线重平衡与灾备演练 | 已完成 | 100% | [scripts/drill-rebalance-dr.sh](../scripts/drill-rebalance-dr.sh)（真实 3 进程 raft：选主→在线重平衡→复制收敛→杀 follower→重启追赶→杀 leader 自动 failover→重启追赶，`=== DRILL PASS ===`）+ `initial_slot_bias`/`shard_map_epoch` 证据 + [L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md) | 按灾备日周期性重跑 |
| P7-02 | 性能回归门禁与 SLO 看板 | 已完成 | 100% | [scripts/check-bench-thresholds.sh](../scripts/check-bench-thresholds.sh)（dict 5000/insert 20000/match 5000000 ns/op 断言 + JSONL 趋势）+ [benchmarks/README.md](../benchmarks/README.md) + CI `bench` 作业已切换硬门禁 | 阈值随基线演进复核 |
| P7-03 | 发布流水线与回滚验证 | 已完成 | 100% | [.github/workflows/ci.yml](../.github/workflows/ci.yml) + `scripts/ci-local.sh` + [L5-systemd-service.md](./L5-systemd-service.md) + runtime/management install scripts + 管理面 smoke + probe latency 阈值门禁 + license 审计 CI 作业 + [L7-release-rollback.md](./L7-release-rollback.md)（代码级回滚/数据级回滚/验证判据） | 发布时按手册执行 |
| P7-04 | 运维手册与证据包 | 已完成 | 100% | [L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md)（7 步判据 + 证据包说明 + 与实现对应关系） | 按阶段产出归档 |

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
| [x] RDF 核心运行时可验收 | 已完成 | L0–L3 + 解析/存储闭环 + **正式验收包**（[R1-acceptance-package.md](./R1-acceptance-package.md)：G1–G5 全 PASS，400 测 / w3c11 492/492 / shacl 97/98 / 内存闭环 / RocksDB reopen 闭环；`bash scripts/acceptance-r1.sh` 可复验；验收中修复 HTTP 结果 JSON 的 IRI 主语渲染缺陷，server 43→44 测） |
| [x] SPARQL 查询基线 | 已完成 | SELECT/ASK/CONSTRUCT + 完整聚合 + Update + 属性路径 + 子查询；**完整 W3C 套件 492/492 全绿**（QueryEvaluation/UpdateEvaluation/Syntax/ResultSet manifest 驱动） |
| [x] 单区域集群核心 | 已完成 | 控制面 + HTTP 演示 + **多节点 raft 数据面 M1–M3 + P4-01–P4-04 落地**（[ADR-0004](../adr/0004-multi-process-raft-data-plane.md)：openraft + HTTP RPC + RocksDB raft CF + 默认运行时切换 + CI 三进程 smoke + 在线重平衡 + 灾备演练 DRILL PASS） |
| [~] 安全与审计基线 | 部分完成 | HTTP 鉴权+审计+JSONL 落盘；**树内 HS256 JWT（OIDC-ready）已落地**；OIDC 完整链路留 R2+ |
| [x] 标准符合性门禁通过 | 已完成 | CI + R1 烟雾 + W3C 子集（required-lite 30/30）+ strict observer + strict readiness 自动评估 + **完整 W3C 套件 manifest 基线 492/492 PASS（fail=0、drift=0）** + SHACL 核心套件 97/98 profile 锁定 |
| [x] 核心 SLO 基线达标 | 已完成 | 实测基线（2026-08-08）：20 样本 success 100%、p95=0ms、max=3ms（阈值 250ms），见 [L5-management-platform-slo.md](./L5-management-platform-slo.md) §5 |
| [x] 恢复演练通过 | 已完成 | RocksDB reopen 单测 + **真实多进程灾备演练**（`drill-rebalance-dr.sh`：杀 follower/杀 leader 自动 failover/重启追赶，DRILL PASS，证据包见 [L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md)） |
| [x] 回滚演练通过 | 已完成 | **实际发布回滚演练 DRILL PASS**（2026-08-08：[`scripts/release-rollback-drill.sh`](../scripts/release-rollback-drill.sh) staging 全流程——V_new=9fac343 部署写入→代码级回滚 V_prev=ec1d539→mv/rm/cp 数据级回滚→恢复 V_new；`/health triples` 1→1→0→1、二进制指纹 2→0→2；手册见 [L7-release-rollback.md](./L7-release-rollback.md) §3.4） |

**R1 判定：** 接近退出标准（约 **95%**；内核+HTTP+集群数据面+符合性+SLO+恢复+发布回滚+正式验收包齐备：W3C 492/492 全绿、多进程 raft 数据面落地、核心 SLO 实测达标、灾备演练 DRILL PASS、实际发布回滚演练 DRILL PASS、R1 正式验收包 G1–G5 全 PASS；剩余缺口：OIDC 完整链路（R2+ 轨））。

### R2

| 检查项 | 状态 |
|--------|------|
| [ ] 代价优化 | 未开始 |
| [ ] OWL 2 RL 核心 | 未开始 |
| [~] SHACL 基线 | 部分完成（`ShaclEngine` 核心约束组件全齐 + 属性路径表达式全量 + W3C SHACL 核心套件接入，`shacl_suite` 97/98 基线 profile 锁定；唯一缺口 uniqueLang-002 词法差异） |
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
| WBS-03 | 存储与事务 | 部分完成 | ~95% | RocksDB 已接 + 纯 CF 索引扫描（SPO/POS/OSP + 命名图 GSPO/GPOS/GOSP）+ 内存/磁盘 MVCC 版本链（跨重启持久） |
| WBS-04 | 查询与优化 | 部分完成 | ~95% | 完整核心代数+优化+绑定 + 完整聚合（GROUP BY/HAVING）+ SPARQL Update 基线 + 子查询（含聚合）+ 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）+ W3C 子集门禁（30/30）；缺高级 Update 形态 |
| WBS-05 | 推理与 SHACL | 部分完成 | ~30% | 前向链推理最小集可用；SHACL 核心约束组件全齐 + 属性路径表达式全量 + W3C SHACL 核心套件接入（97/98 基线，`shacl_suite` profile 锁定；唯一缺口 uniqueLang-002 词法差异） |
| WBS-06 | 分布式运行时 | 部分完成 | ~78% | 控制面增强+HTTP + 数据面同步接口；无多进程数据复制 |
| WBS-07 | API、安全与集成 | 部分完成 | ~87% | 双后端网关+文件审计（哈希链）+Results JSON+ingest+部署脚本+独立管理面 API + ACL/probe；无 TLS/OIDC |
| WBS-08 | 平台工程 | 部分完成 | ~70% | CI workflow + compliance crate + ci-local + systemd 运维文档 + 管理面 smoke + 窗口化 SLO 检查 + 存储微基准（阈值断言 + 趋势记录）+ license 审计 + 在线重平衡/灾备演练脚本 + 发布/回滚手册 |

---

## 6. 质量门禁与治理清单

| 门禁/治理项 | 状态 | 证据 / 缺口 |
|-------------|------|-------------|
| [~] RDF/SPARQL 标准测试 | 部分完成 | `ontolith-compliance` R1 烟雾 17 + W3C 子集运行器（must-pass 30/30，skip=0）+ **完整 W3C 套件 manifest 驱动 runner（`w3c11_suite`，492 条基线：492 PASS / 0 FAIL，fail=0、drift=0）** + CI required-lite / strict observer |
| [~] 故障注入（选主/复制/恢复） | 部分完成 | `ontolith-cluster` 分区注入/愈合与复制路径单测 + **真实多进程灾备演练**（`drill-rebalance-dr.sh`：杀 follower/杀 leader/重启追赶，DRILL PASS） |
| [x] 幂等写入验证 | 已完成 | `InMemoryStorageEngine` 4 测（Put 集合语义去重/重放去重、重复 commit 拒绝、Delete 不存在 no-op、Quad 去重+删除）+ RocksDB 1 测（含索引 CF 一致 + reopen 持久），storage 46→51 测 |
| [~] 性能回归门禁 | 已完成 | `storage_bench` 微基准（字典/写入提交/索引匹配）+ CI `bench` 硬门禁（阈值断言 + 趋势记录，`check-bench-thresholds.sh`） |
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
| ontolith-storage | 内存六索引 + 命名图六置换 + RocksDB 耐久（reopen/abort/delete）+ codec + 命名图匹配 + **幂等写入验证**（Put 去重/Delete no-op/重复 commit 拒绝，内存 4 测 + RocksDB 1 测）（51 测） | `crates/ontolith-storage/src/infrastructure/**` |
| ontolith-transaction | begin/commit/abort、超时清理、active 上限、metrics（7 测） | `crates/ontolith-transaction/src/infrastructure/mod.rs` |
| ontolith-query | SELECT/ASK/CONSTRUCT、JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES、完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）、SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）、子查询基线、属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）、Explain/timeout、租户作用域（`TenantScopedRead/Write` 默认图重指向 + 越权 403）（86 测） | `crates/ontolith-query/src/infrastructure/**` |
| ontolith-parser | N-Triples/N-Quads/Turtle/TriG、流式事件、错误定位、Unsupported 格式、RDF 序列化导出、Turtle 数字字面量完整文法（17 测） | `crates/ontolith-parser/src/infrastructure/**` |
| ontolith-cluster | 选主、分区、复制、commit、rebalance（含 initial_slot_bias 偏斜→均衡）、session sticky、数据面同步 + raft M1（内存存储/传输）+ raft M4（HTTP RPC + RocksDB raft CF + snapshot + 默认运行时切换 + 三进程 smoke + 多进程元数据 RPC + 跨节点数据搬迁 + 真实网络分区）（31 测） | `crates/ontolith-cluster/src/infrastructure/{mod,raft}/**` |
| ontolith-security | disabled/enforced、tenant/user、audit（内存+文件）+ 哈希链验证/篡改检测 + `TenantMode`/`TenantNamespace` 命名空间校验 + **树内 HS256 JWT 验证**（sign/verify、`exp`/`iss`/`aud`、Bearer 鉴权）（18 测） | `crates/ontolith-security/src/{application,infrastructure}/mod.rs` |
| ontolith-observability | sink、导出、采样循环、Prometheus 文本 + **Tracing 全链路**（span 存储/淘汰、确定性 id、W3C `traceparent` 解析生成、线程本地 `TraceScope`、`/admin/traces` JSON 渲染）（11 测） | `crates/ontolith-observability/src/**` |
| ontolith-server | metrics、采样配置、HTTP query decode + 管理面 API/ACL/probe + 强制租户隔离（`ONTOLITH_TENANT_MODE` 写路径盖章/越权 403、读路径 tenant scope、`/health` 姿态）+ **JWT Bearer 鉴权**（`Authorization: Bearer`、伪造/过期 401、JWT 租户优先）+ **Tracing 全链路**（`http.request`/`http.auth`/`sparql.execute`/`data.ingest` span、`Traceparent` 回带、`/admin/traces`）+ **gRPC 网关**（tonic+prost `SparqlService{Query,Health}` HTTP/2、metadata 鉴权、`traceparent` 延续/回带、共享执行路径、`ONTOLITH_GRPC_BIND` 双网关 bin）（33 测） | `crates/ontolith-server/src/{api,bootstrap,grpc,http,management}.rs` |
| ontolith-reasoner | 前向链推理（rdfs5/6/7/8/9 + prp-inv1/2、prp-symp/trp、prp-fp/ifp、cax-sco、cls-svf1/2、cls-avf、cls-int1/2、cls-uni、cls-maxc2、eq-sym/trans、eq-rep-s/p/o、prp-key、prp-spo2 属性链、cls-hv1/2 hasValue、一致性 ⊥ 检测 cax-dw/cls-com/cls-nothing1/2/eq-diff1/2/3 + prp-irp/cax-adc、`InferenceMode` 开关 + 迭代/超时护栏 + `ReasoningReport.inconsistent`）+ SHACL 基线校验（目标选择/核心约束组件全齐（含 languageIn/uniqueLang/xone）/逻辑形状/数值范围/属性对/qualified 计数/语言标签管道/报告/独立与嵌套属性形状/dateTime 比较/词法合法性/自定义 severity/**属性路径表达式全量（inverse/alternative/sequence/zeroOrMore/oneOrMore/zeroOrOne，集合去重 + 闭包护栏）**，80 测；`ShaclEngine` 接入 W3C SHACL 核心套件 97/98） | `crates/ontolith-reasoner/src/infrastructure/{mod,shacl}.rs` |
| ontolith-compliance | R1 烟雾 17 + W3C 子集 profile 1（must-pass 30/30，skip=0）+ 完整 W3C 套件 manifest runner（`w3c11_suite`，492 条基线 profile 锁定）+ W3C SHACL 核心套件 runner（`shacl_suite`，vendored `w3c-shacl/` 121 个 sht:Validate/98 可运行，`w3c-shacl_profile.tsv` 97 PASS/1 FAIL 基线锁定） | `crates/ontolith-compliance/tests/**` |

---

## 7. 变更日志

| 日期 | 作者 | 变更 |
|------|------|------|
| 2026-08-08 | Codex | **R2 退出标准门禁 DONE（PLAN §6 R2 全项勾选）**：新增 `ontolith-compliance` `r2_explain_gate` 5 测（Explain 完整性——BGP/join/filter/union/left join/distinct/aggregate/construct/ask/path/graph 全类别都有 logical/physical steps + algebra summary + 成本估计；成本合理性——selectivity ∈ (0,1]、estimated_rows ≥ 1；代价序——最选择性 BGP 模式排首；语义保持——cost 管线与 rule 管线结果集逐项一致；稳定性——重复 explain 字节级一致）+ `r2_reasoner_gate` 7 测（OWL 2 RL 正确性——subClassOf 传递闭包/domain·range 定型/对称+传递属性/逆属性/功能属性 sameAs/hasValue；不一致检测——disjointWith + AllDifferent members；属性链 prp-spo2 + hasKey prp-key；护栏——迭代上限部分闭包、墙钟 1ms 超时触发且及时返回、40 节点链 741 推理 5s 预算内收敛、Off 模式原样返回）；优化器补全 Path/Graph 成本估计（Path 统一最坏情形 selectivity=1.0，path-only 查询也可出成本），`pattern_signature` 重构为可复用 term/path 签名；CI 新增 `r2-gates` 作业（needs: check，显式跑两门禁）；query 88 测 + compliance +12 测，W3C 492/492 零漂移，clippy 零警告；PLAN §6 R2 范围与退出标准全部 ✅；PROGRESS.md 0.1.35→0.1.36 |
| 2026-08-08 | Codex | **项目进度同步至 GitHub Projects #2 DONE（SYNC-PROJ-0001）**：`docs/github-projects-sync.md` 固化同步契约（认证要求——用户级 Projects v2 不支持 fine-grained PAT，实测 FORBIDDEN，需 Classic PAT `project` scope 或 GitHub App 安装令牌；条目映射表；GraphQL 读写操作；同步流程）；Classic PAT 验证通过后全量同步 44 条：P0–P8 交付物 + R1 退出标准全表 + R2 后续轨，Status（Todo/In progress/Done）与 Priority（P0/P1/P2）按 0.1.34 快照写入并回读验证 total=44；看板 <https://github.com/users/shark8848/projects/2>；后续随 PROGRESS.md 增量按契约 §5 流程同步；PROGRESS.md 0.1.34→0.1.35 |
| 2026-08-08 | Codex | **OIDC 完整链路 R2+ DONE（R1 唯一剩余项勾选完成）**：`crates/ontolith-security/src/infrastructure/oidc.rs`（~760 行，无第三方 JWT 依赖）——RFC 7517 JWKS/JWK（kid+alg 选键、RSA/oct 可用、EC/OKP 解析期过滤 fail-fast）、RS256 自研无依赖大整数 RSA 验签（RFC 7515 A.2.1 官方向量背书；调试期发现样例 signing input 必须保留 JSON 换行缩进 `LA0K…`，Python cryptography 独立复核后修复）、HS256 复用树内 HMAC、`exp`/`nbf`/`iss`/`aud` 策略（`JwtClaims.not_before` + `ONTOLITH_JWT_LEEWAY_SECS`）、RFC 8414 发现文档 issuer 强制匹配、`JwksFetcher`/`CachingJwks`/`JwksVerifier` TTL 缓存刷新（坏响应保留旧钥）；server 接线 `ONTOLITH_OIDC_ISSUER`/`AUDIENCE`/`JWKS_URL`/`CACHE_TTL_SECS`（file:// 快照 + http:// 树内抓取，https 明确拒绝启动并文档化反向代理路径），`HeaderAuthenticator.jwt_oidc` 优先于共享密钥 HS256；`/health`（HTTP+管理面）与 gRPC `HealthResponse` 暴露 `oidc` 姿态；security 18→24、server 44→49 测，workspace 全量 20 套件全绿，clippy 零警告，`--no-default-features` 构建通过；[L5-ontolith-access-security.md](./L5-ontolith-access-security.md) §2 OIDC 小节 + env 契约 + 限制；PROGRESS.md 0.1.33→0.1.34 |
| 2026-08-08 | Codex | **R1 正式验收包 DONE**：`docs/R1-acceptance-package.md`（ACC-R1-0001）+ `scripts/acceptance-r1.sh`（G1 fmt/clippy → G2 workspace 全量 → G3 w3c11 492/492 + shacl 97/98 → G4 内存 INSERT/SELECT 闭环 → G5 RocksDB reopen 持久闭环）首次执行 `=== ACCEPTANCE PASS ===`（20 个 test binary 全 ok 共 400 测、drift=0）。验收中发现并修复 HTTP 结果 JSON 缺陷：`/sparql` SELECT/CONSTRUCT 将存储的 IRI 主语渲染为 bnode（`bound_value_json`/CONSTRUCT 主语渲染未走 `DictionaryCodec::decode_node`，W3C 套件进程内比较有解码故未暴露）；`sparql_results_json`/`bound_value_json` 增字典参数按解码判定 uri/bnode（与引擎 `node_id_term` 同语义），gRPC 调用点同步，新增回归测试 `sparql_http_json_renders_stored_iri_subject_as_uri`（server 43→44 测）；另全仓对齐当前 stable rustfmt 1.9.0 规范格式（24 文件纯格式化）。R1 退出标准全表勾选完成（唯一剩余：OIDC 完整链路 R2+），R1 判定上修至 ~95%；PROGRESS.md 0.1.32→0.1.33 |
| 2026-08-08 | Codex | **P7-03 实际发布回滚演练 DRILL PASS**：`scripts/release-rollback-drill.sh`（入库，staging 隔离，不触碰生产）——V_new=9fac343（含 L7 `shard_map_epoch` 指纹，count=2）→ 部署 + `INSERT DATA` 写入 + 备份 + 重启持久断言（`/health triples=1`）→ 代码级回滚 V_prev=ec1d539（`git archive` + touch 强制重编译，指纹 count=0；数据目录未动，triples 仍 1）→ 数据级回滚（`mv data data-sim-corrupt` 模拟损坏断言 0 → `rm -rf` + 干净 `cp -a` 恢复断言 1）→ 恢复 V_new（triples=1、指纹 2）→ 重建 HEAD 回 `target/release` 并指纹验证；踩坑固化：共享 CARGO_TARGET_DIR mtime 新鲜度陷阱（touch 强制重建）、`grep -q`+pipefail SIGPIPE 偶发误报（改 `grep -c` 计数）、数据恢复用 mv/rm/cp 避免 cp -a 嵌套；记录回填 [L7-release-rollback.md](./L7-release-rollback.md) §3.4，R1 判定上修至 ~90%（剩余：正式验收包、OIDC 完整链路 R2+）；PROGRESS.md 0.1.31→0.1.32 |
| 2026-08-08 | Codex | R1 收尾：幂等写入验证——`InMemoryStorageEngine` 4 测（同批重复 Put 去重、提交后重放去重、重复 commit 拒绝且不重复、Delete 不存在 no-op、Quad 去重+删除幂等）+ `RocksDbStorageEngine` 1 测（重复 Put/重放去重、absent delete no-op、双 delete、索引 CF 一致、reopen 持久）；storage 46→51 测，workspace 全量测试通过，clippy 零警告。核心 SLO 基线达标：真实窗口实测（20 样本 × 1s）success 100%、p95=0ms、max=3ms（阈值 250ms），短窗与天/周窗口评估双通过，基线与结论回填 [L5-management-platform-slo.md](./L5-management-platform-slo.md) §5。R1 退出标准全表勾选：SPARQL 查询基线 / 单区域集群核心 / 标准符合性门禁（492/492 全绿）/ 核心 SLO 基线 / 恢复演练 转已完成，R1 判定上修至 ~85–88%（剩余：正式验收包、OIDC 完整链路 R2+、实际发布回滚演练待首次发布）；PROGRESS.md 0.1.30→0.1.31
| 2026-08-08 | Codex | L7：P7-01/04 在线重平衡与灾备演练——`ClusterConfig.initial_slot_bias` + `apply_initial_slot_bias`（启动时槽位边界右移 bias 个槽，末分片收缩，使首次 rebalance 产生真计划）+ `RaftClusterConfig.initial_slot_bias` 透传 + `ONTOLITH_RAFT_SLOT_BIAS` 环境入口 + `/admin/monitoring`/`/admin/data/rebalance` 增 `shard_map_epoch`（重平衡生效证据；raft 侧 epoch 为 openraft term 不变）；`scripts/drill-rebalance-dr.sh` 真实 3 进程 raft 集群（openraft HTTP RPC + RocksDB raft CF）7 步硬断言：选主→在线重平衡（plans>0 + shard_map_epoch 前进）→复制收敛→杀 follower（多数派提交继续）→重启追赶→杀 leader（自动 failover 提交继续）→重启追赶（`=== DRILL PASS ===`，约 6 秒），证据包 `drill-transcript.txt`；cluster 30→31 测（`rebalance_moves_slots_after_initial_bias`），workspace 全量测试通过，clippy 零警告
| 2026-08-08 | Codex | L7：P7-02 阈值断言与趋势记录——`storage_bench.rs` 增 JSONL 趋势追加（`ONTOLITH_BENCH_TREND_PATH`/`ONTOLITH_BENCH_RUN_ID`，无第三方依赖 civil-date 时间戳）；`scripts/check-bench-thresholds.sh` 跑 bench + 按 case 断言 ns/op 阈值（dict 5000/insert 20000/match 5000000，env 可覆盖）+ 追加趋势；CI `bench` 作业切换为 `bash scripts/check-bench-thresholds.sh`（趋势落到 `benchmarks/trends/storage-bench.jsonl`）；`benchmarks/README.md` 更新阈值/趋势格式/实测基线（dict≈142、insert≈630、match≈722k ns/op）；本地验证全 PASS
| 2026-08-08 | Codex | L7：P7-03 发布/回滚手册——[L7-release-rollback.md](./L7-release-rollback.md)：CI 门禁链（check/w3c-subset/strict/readiness/rocksdb-smoke/bench/license-audit）+ 发布步骤（ci-local → bench → 灾备演练 → release 构建 → systemd 部署 → SLO timers）+ 回滚原则（二进制与数据分离，磁盘格式向前兼容）+ 代码级回滚（tag 检出重建重部署）与数据级回滚（RocksDB BackupEngine 停服恢复）+ 验证判据；[L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md) 演练手册（7 步判据/参数/证据包/实现对应）
| 2026-08-08 | Codex | L6：P6-02 W3C SHACL 核心套件接入——vendored `w3c/data-shapes` 官方核心套件（121 个 `sht:Validate`/98 可运行，`tests/w3c-shacl/core/`）+ manifest 驱动 runner（`tests/shacl_suite.rs`，`shacl_suite` 同 `w3c11_suite` profile 锁定模式，`w3c-shacl_profile.tsv` 84 PASS/14 FAIL 基线）；配套引擎补全：节点形状属性对（`sh:equals`/`sh:disjoint`/`sh:lessThan`/`sh:lessThanOrEquals` 以 focus 自身为值集）、`sh:deactivated true` 跳过、自定义 severity IRI 原样保留、`sh:conforms` 改按 W3C 语义（无任何结果才 true，Warning/Info 计不合规）、`sh:minLength`/`sh:maxLength`/`sh:pattern` 按 SPARQL `str()` 作用于 IRI 全串与字面量词法（bnode 恒失败）、`sh:pattern` 改 search 语义 + `^`/`$` 锚定 + `{n}`/`{n,}`/`{n,m}` 量词（personexample SSN 通过）、`sh:datatype` 词法合法性校验（`"aldi"^^xsd:integer`/`"none"^^xsd:boolean`/byte 越界）、`sh:languageIn` 基础语言范围匹配（`en` 匹配 `en-NZ`）、`sh:uniqueLang` 每 focus 单条无 value 结果、`sh:node` 单条 NodeConstraintComponent 结果、`sh:closed` 不再豁免 rdf:type（由 ignoredProperties 白名单控制）、独立属性形状（shape 自带 `sh:path`）+ 嵌套 `sh:property` 递归（`validation-reports/shared` 双路径同源结果）、xsd:dateTime 带/不带时区比较（混合时区不可比 → 违规）、隐式类目标收敛为 REC 语义（仅形状自身为类）、bnode focus 还原为 BlankNode；SHACL 30→35 测、reasoner 68→73 测，W3C 套件 60→84 PASS（剩余 14：12 项 path/* 属性路径 unsupported + shacl-shacl 元校验 + uniqueLang-002 `"1"^^xsd:boolean` 词法差异），w3c11 492/492 保持全绿，workspace 全量测试通过，clippy 零警告
| 2026-08-08 | Codex | L6：P6-02 属性路径表达式与 shacl-shacl 元校验收尾——`PropertyPath` 域类型（`sh:inversePath`/`sh:alternativePath`/RDF 列表 sequence/`sh:zeroOrMorePath`/`sh:oneOrMorePath`/`sh:zeroOrOnePath`）+ `parse_path`（rdf:first 序列优先、递归路径护栏）+ `path_values` 重写（SPARQL 集合语义去重、双向求值、闭包环路护栏、`sh:zeroOrOne`/`sh:zeroOrMore` 含 focus 自身）+ 结果路径 canonical 序列化（引擎与 `shacl_suite` harness 共用 `PropertyPath::canonical` 比对期望 `sh:resultPath`）；`sh:closed` 白名单改由 `PropertyPath::predicates()` 收集；12 项 `path/*`（alternative/complex×2/inverse/oneOrMore/sequence×2/sequence-duplicate/strange×2/zeroOrMore/zeroOrOne）全转绿，`shacl-shacl` 元校验（以自身为数据/形状）通过；W3C SHACL 核心套件 84→97 PASS/1 FAIL（唯一缺口 uniqueLang-002 `"1"^^xsd:boolean` 词法差异 profile 锁定），SHACL 35→42 测、reasoner 73→80 测，w3c11 492/492 保持全绿，workspace 全量测试通过，clippy 零警告
| 2026-08-08 | Codex | L6：P6-02 SHACL 核心组件补全——语言标签管道重构（`term_key` 语言标签参与 RDF 项相等 → `sh:in`/`sh:hasValue` 区分 `@en`/`@fr`；`literal_string` 改为作用于任意字面量词法形式 → `sh:minLength`/`sh:pattern` 等字符串约束覆盖带语言标签字面量；`focus_as_term` 支持 `lex|rdf:langString|tag` 往返）+ `sh:languageIn`（大小写不敏感标签集合，非字面量/无标签/不在集合均报错）+ `sh:uniqueLang`（同标签重复报错，非标签值不参与）+ `sh:xone`（值节点恰好符合一个形状，0 或 >1 均报错，逻辑形状收尾）；SHACL 21→30 测、reasoner 59→68 测，clippy 零警告，全量测试待本波提交前验证 |
| 2026-08-08 | Codex | L6：P6-03 接入 server 查询/推理管线——`InferenceConfig` 从环境读取（`ONTOLITH_INFERENCE_MODE`/`ONTOLITH_INFERENCE_MAX_ITERATIONS`/`ONTOLITH_INFERENCE_MAX_ELAPSED_MS`，默认 off/64/不限）+ HTTP `?inference=` 每请求模式覆盖（非法 400）；`execute_sparql_with_inference` HTTP/gRPC 共享执行路径：读查询实时以 `ForwardChainReasoner` 物化闭包（Update/Explain 跳过），`ReasoningReadService` overlay 只叠加增量三元组，enforced 租户模式输入限定租户自有命名图 union（双层隔离）；SPARQL 结果 meta 增 reasoning 段（mode/inferred_triples/elapsed_ms/timed_out/inconsistent）；`InferenceMode` 增 as_str/parse；query 86→88 测（execute_planned 共享、update_pipeline_with_read overlay）、server 33→43 测（HTTP 5 + gRPC 1），clippy 零警告，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L6：P6-01 OWL 2 RL 规则扩展收尾——`cls-hv1`/`cls-hv2`（owl:hasValue 双向：定型→值三元组 / 值匹配→定型，含字面量值）、`prp-irp`（IrreflexiveProperty 自反 ⊥）、`cax-adc`（AllDisjointClasses 双类型 ⊥）、`eq-diff2`/`eq-diff3`（AllDifferent owl:members/owl:distinctMembers 同对 sameAs ⊥）、`prp-spo2`（owl:propertyChainAxiom 属性链，任意长度逐跳 join）；eq-diff 编号对齐 W3C OWL 2 Profiles 2012 原表（原 sameAs+differentFrom 冲突由 eq-diff2 改标 eq-diff1，不同From 自反折叠为其推论）；`Rule` 枚举/supported_rules/一致性检测同步，reasoner 52→59 测，clippy 零警告，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L4：P4-02 多进程 Raft M1——openraft 0.9.25 接入（Tier A 锁定，`raft-backend` feature 默认开，`--no-default-features` 回退构建通过）：`TypeConfig`（D=LogPayload/R=LogEntry/u64/BasicNode/TokioRuntime）+ v1 `RaftStorage` 内存存储（`MemStorage`：日志/vote/committed/purged/last_applied/membership/applied，`MemLogReader`/`MemSnapshotBuilder`）经 `Adaptor` 接入 + 内存 `RaftNetworkFactory` 传输（`RaftRegistry` 进程内路由 append/vote/install-snapshot）+ `RaftClusterRuntime` 集群 trait 适配器（`ElectionService`/`Replicator`/`MetadataService` 核心由 openraft 背书：`client_write`→commit、metrics 映射 leader/epoch/index；`ShardRouter`/`Rebalance`/`DataPlaneSync`/`FaultInjector` 委托模拟器）；单节点 `bootstrap` 自选主、双节点内存传输选举+复制通过；cluster 17→21 测，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L4：P4-02 多进程 Raft M2——多进程 HTTP RPC + RocksDB raft CF + snapshot install（ADR-0004 决策 2/3）：openraft 开 `serde` feature（`serde`/`serde_json` 直依赖，域类型 `LogPayload`/`LogEntry`/`ShardId`/`ClusterEpoch` 增 `Serialize`/`Deserialize`）；树内最小 HTTP/1.1 RPC（`crates/ontolith-cluster/src/infrastructure/raft/http.rs`，沿用 L5 `ontolith-server::http` 同风格，未引入 axum/reqwest）：`HttpRaftServer`（`/internal/raft/{vote,append-entries,install-snapshot}` + `Authorization: Bearer <secret>` 共享 secret 认证，raft 错误经 serde_json 往返为 `RemoteError`）+ `HttpRaftClient`/`HttpRaftFactory`（`RaftNetworkFactory::new_client` 依 `BasicNode.addr` 建 HTTP 客户端，`spawn_blocking` 承载阻塞 IO）；RocksDB 独立 `raft` CF：`RocksDbStorageEngine::raft_cf_*` 字节级原语（put/put_batch/`RaftCfOp` 原子 batch/get/delete/delete_range/scan_range/scan_prefix）+ `RocksRaftStorage` v1 `RaftStorage`（日志/vote/committed/last_purged/log_last/membership/应用态/快照 meta+字节，`RocksLogReader`/`RocksSnapshotBuilder`，snapshot `build_snapshot`→`install_snapshot` 原子替换 `applied/*` + 快照引用）；`RaftClusterConfig` 增 `http_listen_addr`/`raft_secret`/`raft_storage_path`，`RaftClusterRuntime` 配置化选择存储与传输（内存回退保留）；cluster 21→26 测（RocksDB 日志 append/read/purge/delete-conflict + snapshot build/install 往返、HTTP 共享 secret 401 拒绝、HTTP install-snapshot RPC 往返、双节点 HTTP+RocksDB 选举/复制/落盘），clippy 零警告，`--no-default-features` 回退构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L4：P4-02 多进程 Raft M3——默认运行时切换 + CI 三进程 smoke（ADR-0004 决策 5）：`AppState.cluster` 由具体 `Arc<InMemoryClusterRuntime>` 改 `Arc<dyn ClusterRuntime>`（9-trait 复合 trait，`InMemoryClusterRuntime`/`RaftClusterRuntime` 均实现；新增 `new_memory_with_cluster`/`new_rocksdb_with_cluster` 构造器，`from_parts` 改 `pub(crate)`）；管理二进制 `ONTOLITH_CLUSTER_MODE` 默认 `raft`（内存模拟器降级为 测试/CI harness，`memory` 显式选择），raft 模式经 `ONTOLITH_RAFT_NODE_ID`/`ONTOLITH_RAFT_LISTEN`/`ONTOLITH_RAFT_SECRET`/`ONTOLITH_RAFT_MEMBERS`/`ONTOLITH_RAFT_STORAGE_PATH` 配置三进程固定成员引导（多节点同成员 `initialize` 容忍 `NotAllowed` 安全忽略，openraft 文档背书）；`Replicator::replicate_to_followers` 真实语义——leader `replication` metrics 水印增量（`replicated_watermark` 逐 follower 记录 acked index，返回本轮新增 acked 条数）、`replicate_to_followers_respecting_partition` 排除被分区隔离节点、`applied_index` 返回 follower acked index；`/admin/data/replicate?append=1` 支持通过管理 API 驱动 raft 写入；cluster 26→27 测（三节点 HTTP+RocksDB 同成员引导、多数派提交、`replicate_to_followers>=1`、失一 follower 后仍可提交）；CI `check` job 新增 multi-node raft smoke（3 进程启动、leader 观测、append→全节点 commit 推进、杀 follower 后多数派继续提交），`ontolith-server` 增 `raft-backend` feature（默认开，`--no-default-features` 优雅报错）；clippy 零警告，workspace 全量测试通过（drift=0） |
| 2026-08-08 | Codex | L4：P4-01 多进程元数据服务与主从选举收尾——`LogPayload` 增 `RegisterNode(ClusterNode)`/`Heartbeat{node_id,tick}`/`SetNodeStatus{node_id,status}` 元数据变异变体，`ClusterNode`/`ClusterNodeId`/`RegionId`/`NodeRole`/`NodeStatus` 增 `cfg_attr(raft-backend)` serde derive；`RaftClusterRuntime` 增复制式节点注册表（`nodes: RwLock<HashMap>` + `applied_watermark`，`sync_applied()` 把 applied 日志增量折叠进注册表——幂等、重启后从 RocksDB applied 条目重建）；元数据变异统一走 `metadata_mutation`：leader 本地 `client_write` 提交，follower 经新增 `/internal/raft/apply` HTTP RPC （`HttpRaftServer` 409 携带 leader 提示）转发并最多重试 3 次；`membership()`/`status()` 改读复制式注册表（role 按当前 leader 刷新，bootstrap 以固定成员列表种子化）；cluster 27→28 测（三节点 HTTP+RocksDB：follower 发起 register/heartbeat/set_node_status 全节点收敛、leader 直发路径、status().node_count 5），clippy 零警告，`--no-default-features` 回退构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L3：P3-05 第十波——syntax-query 全收尾 + 套件全绿：负向语法 8 项（`SELECT *`+GROUP BY 拒绝、尾随内容校验（`SELECT COUNT(*) {}`/BINDINGS）、组中段子查询拒绝且组首子查询放宽（修复 sq11-14、SAMPLE、Post-subquery VALUES、delete-insert 4、basic-update bnode 4 共 13 项误拒绝）、前缀名数字开头、`:c\:z` 非法转义、字符串 `\uD800` 部分代理对拒绝——`unescape` 完整 `\u`/`\U` 解码 + 代理对/位数校验）+ 正向 2 项（syn-pname-09：`::`/`z::` 前缀与 `_12.3_` 本地名尾点剥离、syntax-select-expr-04：空前缀函数 `:fn(...)`）；`FROM`/`FROM NAMED` 数据集子句（§18.2.1，查询默认/命名图数据集）+ `CONSTRUCT FROM <g> WHERE` 简写（模板即模式）+ 组内 FILTER 延后到组末尾应用（bind08）+ MergedGraphRead 无 USING NAMED 不再回退全命名图（delete USING 2）+ STR 解码 Node/Blank 为 IRI（negation）；harness：`load_data` 查询/更新数据图暴露区分、qt:data 裸文件名 base、RDF/XML 数据读取、整数字面量族扩充；query 83 测，W3C 458→492 PASS（+34，fail=0、drift=0，492/492 全绿） |
| 2026-08-08 | Codex | L4：P4-03 跨节点数据搬迁——`DataPlaneSync for RaftClusterRuntime` 从模拟器语义升级为真实实现：新增 `DataPlaneSnapshotIo` trait（`export_snapshot`/`import_snapshot`，`Send+Sync`，由进程接入 L2 存储）与 `/internal/raft/transfer-snapshot` HTTP RPC（`TransferSnapshotRequest{shard_id,slots_start,slots_end,snapshot_id,bytes}`，Bearer 认证，目标无 hook 返回 503）；`complete_transfer` 从调用节点本地 IO hook 导出快照字节、校验目标非分区且在 raft 成员表（失败返回 `OntolithError`）、POST 至目标 `/internal/raft/transfer-snapshot` 导入，200 返回 `SyncReceipt`（`transferred_entries=snapshot_id`、`completed_at_epoch=current_epoch`），无 hook 时保留模拟回执回退（不触网）；`RaftClusterRuntime` 与 `HttpRaftServer` 共享 `data_plane_io` Arc，`set_data_plane_io` 注入 hook；cluster 28→29 测（三节点 HTTP+RocksDB：快照字节经 HTTP 迁移到目标并记录 shard/slots/bytes、无 hook 节点仍出模拟回执），clippy 零警告，`--no-default-features` 回退构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L4：P4-04 真实网络分区——`FaultInjector for RaftClusterRuntime` 升级：`inject_partition` 把 `<ClusterNodeId>` 映射为 raft id 集合写入 `partition`（`HttpRaftFactory::new` 注入，各节点客户端共享同一 Arc），`HttpRaftClient::post` 在 target 或 self 处于分区集合时返回 `RPCError::Network`（raft 层对称丢弃而非 HTTP 超时）；`metadata_mutation` 拒绝被隔离节点自身操作（`node is isolated by a network partition`）与转发到被隔离 leader（`leader is isolated...`），`complete_transfer` 拒绝迁移到被分区目标；`heal_partition` 清空集合并委托内层模拟器；cluster 29→30 测（三节点 HTTP+RocksDB：隔离 leader+一 follower 使多数派不可达→元数据转发确定性失败、heal 后重新选主与转发恢复、n9 注册全节点收敛），clippy 零警告，`--no-default-features` 回退构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L5：P5-03 强制分库/行级租户隔离——`ontolith-security` 增 `TenantMode`（`ONTOLITH_TENANT_MODE=enforced`）与 `TenantNamespace`（`urn:tenant:<t>` 命名空间，`is_owned`/`require_owned`，`urn:tenant:acme2` 不属于 `acme`），+3 测；`ontolith-query` 增 `QueryRequest.tenant_scope` + `TenantScopedRead`/`TenantScopedWrite` 执行器视图：默认图重指向租户图、命名图可见性限制在租户命名空间、更新默认图写自动盖章进租户图；`FROM`/`GRAPH`/`USING`/图管理目标等显式图引用越权返回 `forbidden`（HTTP 403），+3 测；`ontolith-server` 强制模式写路径全部盖章（`?graph=` 越权 403、TriG 命名图越权 403）、读路径注入 tenant scope、`/health`/`/admin/config` 暴露 `tenant_mode`，+2 测；security 9→12、query 83→86、server 22→24 测，clippy 零警告，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L5：P5-02 OIDC/JWT 鉴权——树内 HS256 JWT 验证（RFC 7519 子集：base64url RFC 4648 §5 + SHA-256 FIPS 180-4 + HMAC-SHA256 RFC 2104 + 常量时间签名比对，FIPS/RFC 4231 向量背书；`sign_hs256`/`sign_tenant_token`/`verify_hs256` + `JwtVerifyOptions` `iss`/`aud` 策略 + `auth_context_from_claims` 租户优先）；`HeaderAuthenticator` 增 `authenticate_with_bearer`（`ONTOLITH_JWT_SECRET`/`ONTOLITH_JWT_ISSUER`/`ONTOLITH_JWT_AUDIENCE`，Bearer 存在时走 JWT、否则回退 Header/API-Key）；server `auth()` 读 `Authorization: Bearer`、`/health` 暴露 `jwt` 姿态；security 12→18 测（FIPS/RFC 4231 向量、sign/verify 往返、篡改/过期/iss/aud 拒绝、bearer 鉴权）、server 24→26 测（Bearer 认证、伪造/过期 401、JWT 租户优先盖章），clippy 零警告，`--no-default-features` 构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L5：P5-05 Tracing 全链路——`ontolith-observability` 追踪域模型（`TraceId`/`SpanId`/`SpanEvent`/`SpanStatus`/`TraceContext`）+ `InMemoryTraceStore`（1024 cap 逐旧）+ 确定性 128-bit trace/64-bit span id + W3C `traceparent` 解析/生成 + 线程本地 RAII `TraceScope`（子 span 埋点无需改 handler 签名）；server 网关 `http.request` 根 span + `http.auth`/`sparql.execute`/`data.ingest` 子 span（父链/状态/属性）、响应回带 `Traceparent`、`/health`/`/admin/config` 暴露 `tracing` 姿态、`GET /admin/traces`（按 trace 分组、新→旧、limit）；observability 6→11 测、server 26→29 测（全链路父链与回带、失败请求 error 状态、`/admin/traces` 列表），clippy 零警告，`--no-default-features` 构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L5：P5-01 gRPC 网关接入——tonic 0.12 + prost 0.13 + `protoc-bin-vendored` 3（`grpc-backend` feature 默认开，`--no-default-features` 回退构建通过）：`proto/ontolith/v1/sparql.proto` `SparqlService{Query,Health}`（`QueryRequest{query,format,explain,timeout_ms,consistency}`/`QueryResponse{ok,http_status,body,error}`/`HealthResponse{status,backend,tenant_mode,auth_mode,jwt,tracing}`）；`SparqlGateway` 复用 HTTP 网关共享执行路径（`execute_sparql`/`explain_sparql`/`explain_json`/`sparql_results_json` 提为 `pub(crate)`）+ 同构鉴权（metadata `x-ontolith-tenant`/`x-ontolith-user`/`x-api-key`/`authorization`，enforced 401/跨租户 403）+ W3C `traceparent` 延续/回带 + `http.auth`/`sparql.execute`/`grpc.query`/`grpc.request` 根/子 span；`serve_grpc` 独立 tokio multi_thread runtime 线程；`ontolith-server` bin bootstrap 由 metrics 演示升级为真实双网关（HTTP `ONTOLITH_BIND` + gRPC `ONTOLITH_GRPC_BIND` 默认 `127.0.0.1:50051`，共享 env 契约 `build_gateway_app_state_from_env`）；server 29→33 测（roundtrip insert/select、enforced 401、跨租户 403、health），clippy 零警告，`--no-default-features` 构建通过，workspace 全量测试通过，drift=0 |
| 2026-08-08 | Codex | L3：P3-05 第九波——聚合补全与结果比对增强：表达式聚合参数（`AVG(IF(isNumeric(?p), ?p, COALESCE(xsd:double(?p),0)))` 等任意表达式逐行求值，裸变量走快速路径 `AggregateExpr`）+ 投影 `(expr AS ?alias)` 内嵌聚合提升（`(MIN(?p)+MAX(?p))/2`）与 HAVING 多约束循环（`HAVING (COUNT(*) > 1) (COUNT(*) < 3)`）；SUM decimal 精确累加（scaled-i128 无外部依赖，`1.0+2.2+3.5+2.2+2.2=11.1`，agg-sum-01 转正）；AVG 空组定义 0（agg-avg-03）、非数值/错误参数整组无绑定（agg-err-01）、十进制先精确除再转 f64（`11.1/5=2.22`）；MIN/MAX 接入 SPARQL 全序（bnode<IRI<literal，agg-err-01 `?c` 表达式随之转错误）；GROUP_CONCAT 恒为简单字面量 + DISTINCT 去重（agg-groupconcat-4/5/6/distinct 转正）；harness 新增 rs:ResultSet Turtle 结果表比对（相对 IRI 按文件目录 base 解析并归一为文件名）+ graphData 直接 IRI/无 label 回退图名；query 76→83 测，W3C 444→458 PASS（+14：aggregates 9、property-path 3、bindings 1、negation 1），drift=0 |
| 2026-08-08 | Codex | L3：P3-05 第八波——查询/模板语法补全：空白节点属性列表独立模式与对象位置展开（`[ :p ?o ]`，谓词可为属性路径 `[ :p\|:q ?x ]`）+ RDF 集合 `( item ... )` 解析为 rdf:first/rest 链（查询模式与 CONSTRUCT 模板，空集合 → rdf:nil）+ CONSTRUCT/update 模板空白节点按解逐行实例化（模板标签未绑定则每解铸新 bnode，CONSTRUCT WHERE 保留已匹配节点）+ 子查询 SELECT 投影表达式 `(expr AS ?alias)`（含聚合歧义识别、`SELECT *` 空投影）与 §18.2.2 变量作用域校验（子查询不得投影外层 SELECT 表达式别名，scope2 负向保持、scope1/3 正向转正）+ 模板/更新物化 NodeId→Term 字典解码（IRI 宾语不再误作空白节点，delete-insert 1/5b 转正）+ `QueryReadService::decode_node` 字典桥 + BGP 绑定 Node(id)≡Blank(id) 等价（sq11/13 语义转正）+ harness 图比对 bnode 标签位置化归一；query 76 测，W3C 434→444 PASS（+10：subquery 3、syntax-query 4、construct 列表 1、delete-insert 2），drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第七波——SPARQL Update 图管理与更新语义收尾：ADD/COPY/MOVE/CREATE（SILENT + GraphOrDefault 可选 GRAPH + 目标图替换语义，MOVE 先清目标）解析与执行；modify 模板与 DATA 块支持 `GRAPH <g> { }`、`USING [NAMED]` 数据集子句、对象列表/单引号字面量；相对 IRIREF（无 scheme）解析放宽（`<s>` 等，IRI 含空白仍拒绝）；请求级多操作 `;` 分隔 + 尾随内容校验（bad-07/08/09 转负）+ DELETE 模板 bnode 拒绝（delete-insert 3/5/6/7/8/9 转负）+ INSERT/DELETE DATA bnode 标签 request 级作用域（syntax-update-53 正、54 负）；命名图 pending 读取贯通（`quads_by_graph_in_txn`/`named_graph_quads_in_txn`/`all_in_txn` + UpdateWriteService/QueryReadService txn 感知），请求内更新顺序可见性（basic-update 4 项 bnode 语义转正）；query 76 测，W3C 339→434 PASS（+95：add 8、copy 7、move 6、delete/delete-insert/delete-data/clear/drop/basic-update 30+、syntax-update 12、post-VALUES 7、相对 IRI 等），drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第五波——哈希函数族与字符串函数/词法收尾：自包含 MD5/SHA-1/SHA-2 家族 + REGEX/REPLACE/BNODE/UUID/STRUUID + 表达式 Node/IRI 值相等 + harness 空数据提交修复 + 相对 IRIREF 按 BASE 解析（functions 全绿）；query 67 测，W3C 284→322 PASS，drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第六波——属性路径补全：SPARQL Negated Property Set（`!(p|^q|a)` 解析 + 数据驱动对级求值，集合非空才激活对应方向）+ `^(` 整路径取反 + 零长路径语义（`?`/`*` 匹配模式显式端点常量、空数据集自配对）+ `parse_word` 修正（裸 `?` 为路径修饰符边界、PN_LOCAL_ESC `\?` 转义）+ 零长路径纳入字面量宾语（`foaf:knows*` 含 `"test"` 自配对）；query 73→76 测，W3C 322→339 PASS（+17：property-path 10、零长常量端点 3、NPS 4），drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第四阶段——compliance harness 与字符串函数语义收尾：harness 数值归一（decimal/double 按 f64 bits 值比较、float 展宽 f64、XSD 布尔词法归一）+ 结果比对 bnode 标签位置化映射（标签无关）+ `ExecCtx` 注入 BASE、`IRI()/URI()` 相对 IRI 解析 + `CONCAT` 全参数同 lang 才保 lang（混合或无 lang → plain）+ `STRBEFORE/STRAFTER` 按 SPARQL 参数兼容语义（两 simple/xsd:string、同 lang、左 lang+右 simple，否则错误；命中保左 lang 含空串参数、未命中 plain 空串）+ `STRDT/STRLANG/LANGMATCHES/TIMEZONE` 错误语义；query 67 测，W3C 265→284 PASS（+19：cast 6、functions 11、csv-tsv-res 2），drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第三阶段——完整 datatype/lang 字面量模型（自底向上 L0→L5）：`LiteralValue` 增 `Lang`/`Typed`/`Float`/`Double` 变体（核心规范化编码新 tag，旧盘兼容）；Turtle/N-Triples/SPARQL 查询三处解析保留 `@lang` 与 `^^datatype`（Turtle 词法修正：指数→double、含点无指数→decimal）；RocksDB term codec 增 7-10 tag；查询求值按 SPARQL 语义重写——XPath 数值提升算术（int/int 除法→decimal、float/double 传播）、RDFterm-equal 值相等（跨数值类型、simple≡xsd:string）、比较（数值提升/字符串/lang 排序）、完整 CAST（integer 截断、decimal/double/float 规范词法校验、string 词法门控）、字符串函数 datatype 规则（UCASE/LCASE/SUBSTR/STRAFTER/STRBEFORE 保 lang，CONCAT 同 lang 合并，STRLEN→integer）、STRDT/STRLANG/LANG/DATATYPE、ABS/CEIL/FLOOR/ROUND 保类型、ENCODE_FOR_URI、NOW/RAND、日期函数 YEAR/MONTH/DAY/HOURS/MINUTES/SECONDS/TIMEZONE/TZ（xsd:dateTime 解析 + dayTimeDuration/TZ 输出）、IN/NOT IN 错误语义；聚合 SUM/AVG 数值等级传播、GROUP_CONCAT lang 规则；server JSON 序列化输出 datatype/xml:lang；compliance 数值 lex 按值归一 + 新变体比对；query 66→67 测，W3C 229→265 PASS（+36：cast 6、functions 27、aggregates 9、negation/grouping 2、syntax 2），drift=0 |
| 2026-08-07 | Codex | L3：P3-05 第二阶段 W3C 欠账提 PASS——EXISTS/NOT EXISTS/MINUS（`Expression::Exists` 带当前绑定求值、`Algebra::Minus` 共享变量差集）+ 聚合扩展（GROUP_CONCAT/SEPARATOR、SAMPLE、SUM/AVG/MIN/MAX DISTINCT、多 HAVING）+ CAST（`xsd:` 前缀函数与 `CAST(expr AS xsd:…)`）+ CONSTRUCT WHERE（模板即模式）+ 构造/查询语法（`;`/`,` 简写、`[]` 空白节点、BIND scope 只认真正绑定）+ IRI subject 经字典桥匹配（`bind_pattern`/`bound_node`/模板实例化）+ 表达式求值接入抢占 ctx；negation 11→1、aggregates 12→3、construct 逗号/WHERE 转正、syntax EXISTS/MINUS/BIND 全过；query 65 测，W3C 192→229 PASS（parse-error 109→56，drift=0） |
| 2026-08-07 | Codex | L3：P3-04 异步抢占 token——`PreemptionToken`（墙钟 deadline + 共享 cancel 标志，`reason()` 区分 Timeout/Cancelled，`preempt()` 跨线程触发）；执行器轮询粒度细化到 BGP 候选/join 行/FILTER/EXTEND/VALUES 行；Update 抢占返回 `timed_out`/`cancelled` 且不落写；query 59→63 测 |
| 2026-08-07 | Codex | L3：P3-03 HTTP Explain API 成本信息——`QueryExplain`/`QueryPlan` 新增 `estimated_rows`（主导 BGP 乘积×总数）与 `pattern_costs`（逐模式 selectivity/estimated_rows，代价优化器填充）；HTTP `/explain` JSON 输出两字段；query 58→59 测，server 19→20 测 |
| 2026-08-07 | Codex | L3：P3-02 代价模型/统计——`QueryStatistics` 契约（triple/subject/predicate/object 计数 + 均匀选择性估计）、`EngineQueryStatistics`（引擎增量统计）、`CostBasedOptimizer`（BGP 贪心 join 序 + 绑定传播，语义保持）；`update_pipeline`/新增 `cost_pipeline` 接入代价优化；query 55→58 测，W3C 套件基线无漂移 |
| 2026-08-07 | Codex | L3：P3-01 SPARQL Update 高级形态——`CLEAR/DROP [SILENT] DEFAULT/NAMED/ALL/GRAPH <g>`、`WITH <g>` 图作用域 DELETE·INSERT…WHERE / DELETE WHERE（WHERE 以图 g 为默认图、模板写入图 g）、`LOAD [SILENT] <src> [INTO GRAPH <g>]` 本地命名图复制子集；修复空更新（无匹配）误报 “pending storage transaction not found”（无写入不提交）；`UpdateWriteService` 增图范围读取；query 46→55 测，W3C 套件基线 127→151 PASS（24 项 FAIL→PASS，无回归） |
| 2026-08-07 | Codex | L2：P2-05 可恢复耐久写入路径——`RocksDbOptions`（`sync_writes` 默认 true：commit/delete/字典/WAL 追加走 `WriteOptions::set_sync` fsync）+ `open_with_options`；RocksDB BackupEngine `create_backup`（提交锁串行化 + flush 后快照）/`restore_backup` 演练（MVCC 版本随备份恢复，命名图 quads 一并保留）；storage 43→46 测，全量测试通过（server 2 测因沙箱禁绑端口除外） |
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
| 2026-08-07 | Codex | L2：P2-02 内存 MVCC 版本链——`StorageState` 改为不可变提交快照版本链（`versions: BTreeMap<u64, Arc<CommittedGraph>>`，0=genesis）+ `next_version`；`SnapshotRef` 携带 `version`，快照 pin 防剪枝；`prune_versions`/版本保留策略（保留最新+retention+pin+genesis）；`triples_at_version_in_txn`/`quads_at_version` 按版本读取（剪枝回退最旧保留版）；`delete_by_key` 仅在有删除时铸新版本；WAL 重放重建版本链；修正 `prune_locked` 链首 genesis 阻断剪枝的 bug；storage 30→35 测，全量 260 测通过（server 21 测需端口）。 |
| 2026-08-07 | Codex | L2：P2-02 续——repo 层按版本读取接入——`TripleRepository` 新增 `all/by_subject/by_predicate/by_object/matching_at_version_in_txn`、`QuadRepository` 新增 `all_at_version`/`by_graph_name_at_version`（非 MVCC 引擎默认回退最新）；`InMemoryTripleRepository`/`EngineTripleRepository`/`InMemoryQuadRepository` 覆写走引擎版本链；storage 35→38 测，全量 263 测通过。 |
| 2026-08-07 | Codex | L2：P2-01 纯 CF 索引扫描——RocksDB 新增 SPO/POS/OSP 索引列族（RFC-0001 §4 键格式），读路径改为 CF 前缀扫描（by_subject→SPO、by_predicate→POS、by_object→OSP、图 quads→graph 前缀），删除磁盘→内存六置换索引重建（`EngineState` 仅剩 pending_writes）；`DeleteKey` 预镜像改由 SPO CF + quads CF 扫描；stats 直接 CF 统计；旧库打开自动回填索引 CF；移除死代码 `TripleIndexes`/`GraphIndex`（内存引擎自有结构）；storage 38→37 测（-4 死代码测试 +3 CF 扫描测试），全量 262 测通过。 |
| 2026-08-07 | Codex | L2：P2-02 RocksDB 磁盘 MVCC 版本链（跨重启持久）——新增 `versions`/`versions_quads` 快照 CF（键 = BE version ‖ 物理键，前缀扫描隔离版本），`meta.next_version` 持久化；提交/`delete_by_key` 铸全量不可变版本快照；剪枝保留最新+retention+pin（genesis 0 隐式）；`committed_version`/`version_count`/`pruned_version_count`/`pinned_snapshot_count`/`release_snapshot`/`prune_versions`/`triples_at_version_in_txn`/`quads_at_version` 全实现（剪枝后回退最旧保留版本）；`snapshot_with` 捕获并 pin 当前版本；旧库（P2-01 及更早）打开自动回填 v1 快照（空库不偏移版本号）；storage 37→40 测，全量 265 测通过。 |
| 2026-08-07 | Codex | L2：P2-04 命名图六置换索引——`domain/encoding.rs` 新增 GSPO/GPOS/GOSP 键与前缀编码器（graph‖S/P/O 置换）；RocksDB 新增 `gspo_index`/`gpos_index`/`gosp_index` CF，PutQuad/DeleteQuad/DeleteKey 同步维护；`quads_matching_in_graph` 按最选择性绑定位置前缀扫描（graph+s/p/o），图内 quads 不再全扫；旧库打开自动回填命名图索引 CF；storage 40→43 测（+1 编码前缀 +2 索引读写/删除/重开），全量 268 测通过。 |

---

## 8. 近期行动队列（可勾选）

### 后续执行队列（自底向上，逐层到顶层应用）

原则：先底层逐层到最顶层应用——优先完成当前最低未完成层，再推进上一层；避免跳层开发。R1 退出标准收尾（核心 SLO 基线、恢复/回滚演练、全表勾选）随各层推进同步完成。

> 当前光标：**L7 平台工程（P7-01/02/03/04 已完成，收尾提交中；下一步 L8 AI-Native）**

- [x] **L0/L1 底层契约**：P1-02 并发字典契约、P1-03 存储接口版本冻结、P1-04 独立编码 RFC + 磁盘布局（2026-08-07）
- [x] **L2 存储内核**：P2-02 真 MVCC 版本链（内存+磁盘）✅（2026-08-07，storage 30→40 测）、P2-01 纯 CF 索引扫描 ✅（2026-08-07）、P2-04 命名图六置换 ✅（2026-08-07；Async 维护预留）、P2-05 fsync/备份演练 ✅（2026-08-07，storage 43→46 测）
- [~] **L3 查询引擎**：P3-01 高级 Update ✅（2026-08-07，query 46→55 测，W3C 127→151 PASS）、P3-02 代价模型/统计 ✅（2026-08-07，query 55→58 测）、P3-03 HTTP Explain API ✅（2026-08-07，query 58→59 测）、P3-04 异步抢占 token ✅（2026-08-07，query 59→63 测）、P3-05 标准符合性子集已完成 ✅ 十阶段（2026-08-08，W3C 169→229→265→284→322→339→434→444→458→492 PASS：函数/投影/算术 + EXISTS/MINUS/聚合扩展/CAST/构造语法 + 完整 datatype/lang 字面量模型 + harness 数值归一/相对 IRI/字符串函数兼容语义 + 哈希/REPLACE/BNODE/正则/UUID/属性路径全量 + 图管理 ADD/COPY/MOVE/CREATE/USING/命名图数据/请求内更新可见性 + 第八波 空白节点属性列表/RDF 集合/子查询投影 + 第九波 聚合补全/rs:ResultSet/graphData 直接 IRI + 第十波 syntax-query 收尾/数据集子句/FILTER 延后/Unicode 转义）→ 欠账清零（fail=0、drift=0）
- [x] **L4 集群**：P4-02 多进程 Raft **M1–M3 完成**（2026-08-08）；P4-01 多进程元数据 RPC **完成**（`/internal/raft/apply`：register/heartbeat/set_node_status 经 raft 提交、全节点复制注册表收敛，cluster 27→28 测）；P4-03 跨节点数据搬迁 **完成**（`DataPlaneSnapshotIo` + `/internal/raft/transfer-snapshot` 真实字节迁移，cluster 28→29 测）；P4-04 真实网络分区 **完成**（`HttpRaftClient` 对称丢弃分区 RPC + `metadata_mutation` 隔离拒绝/愈合恢复，cluster 29→30 测）→ **L4 全绿，光标移至 L5 接入与安全**
- [x] **L5 接入与安全**：P5-01 gRPC 网关 **完成**（tonic+prost `SparqlService{Query,Health}` 真实 HTTP/2 + metadata 鉴权 + `traceparent` 延续/回带 + 双网关 bin，server 29→33 测）；P5-02 OIDC/JWT **完成**（树内 HS256 Bearer 鉴权 + `iss`/`aud` 策略 + JWT 租户优先，security 12→18 测）；P5-03 强制租户隔离 **完成**（`ONTOLITH_TENANT_MODE=enforced`：`TenantNamespace` + 执行器租户视图 + 写盖章/越权 403，query 83→86 测）；P5-05 Tracing 全链路 **完成**（`traceparent` 延续 + 根/子 span + `Traceparent` 回带 + `/admin/traces`，observability 6→11、server 26→29 测）→ **L5 全绿，光标移至 L6 推理与验证（P6-01 规则扩展收尾 → P6-03 接入 server 查询/推理管线 → P6-02 SHACL 补全）**
- [x] **L6 推理与验证**：P6-01 规则扩展 **完成**（cls-hv1/2、prp-irp、cax-adc、eq-diff2/3 AllDifferent、prp-spo2 属性链，reasoner 52→59 测）；P6-03 接入 server 查询/推理管线 **完成**（`InferenceConfig` + `ONTOLITH_INFERENCE_*` 环境 + HTTP `?inference=` 覆盖 + `ReasoningReadService` overlay + reasoning meta + 租户隔离，server 33→43 测）；P6-02 SHACL 核心组件补全 **完成**（languageIn/uniqueLang/xone + 语言标签管道重构，SHACL 21→30 测、reasoner 59→68 测）→ W3C SHACL 套件接入 **完成**（vendored 官方 core 套件 + `shacl_suite` runner + `w3c-shacl_profile.tsv` 84 PASS/14 FAIL 基线：60→84 PASS，SHACL 30→35、reasoner 68→73 测）→ 属性路径表达式与 shacl-shacl 元校验 **完成**（`PropertyPath` 全量：inversePath/alternativePath/sequence/zeroOrMore/oneOrMore/zeroOrOne + canonical 结果路径比对；12 项 path/* 与 shacl-shacl 转绿，84→97 PASS/1 FAIL 基线：唯一缺口 uniqueLang-002 词法差异，SHACL 35→42、reasoner 73→80 测）→ **L6 全绿，光标移至 L7 平台工程（P7-01/04 重平衡与灾备演练）**
- [x] **L7 平台工程**：P7-01/04 在线重平衡与灾备演练 **完成**（2026-08-08：`drill-rebalance-dr.sh` 真实 3 进程 raft 走完 7 步 DRILL PASS：选主→在线重平衡（slot bias 偏斜 + `shard_map_epoch` 前进证据）→复制收敛→杀 follower（多数派提交）→重启追赶→杀 leader（自动 failover）→重启追赶；cluster 30→31 测）✅；P7-02 阈值断言/趋势记录 **完成**（2026-08-08：`check-bench-thresholds.sh` 按 case ns/op 硬断言 + JSONL 趋势，CI `bench` 作业已切换）✅；P7-03 发布/回滚手册与**实际演练** **完成**（2026-08-08：[L7-release-rollback.md](./L7-release-rollback.md) 代码级/数据级回滚 + 验证判据 + §3.4 实际演练记录；`release-rollback-drill.sh` staging DRILL PASS）✅；P7-04 运维手册与证据包 **完成**（[L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md)）✅ → **L7 全绿，光标移至 L8 AI-Native**
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
- [x] `benchmarks/` 性能基线用例（P7-02）：`storage_bench` 微基准 + `benchmarks/README.md` + CI `bench` 作业 + **阈值断言与趋势记录**（`check-bench-thresholds.sh`，2026-08-08）
- [x] 核心 SLO 基线达标（R1 检查项）：实测基线 2026-08-08——20 样本 success 100%、p95=0ms、max=3ms（阈值 250ms），见 [L5-management-platform-slo.md](./L5-management-platform-slo.md) §5

**R1 退出标准剩余项**

- [x] 多节点数据面设计定稿：ADR-0004（openraft behind traits + 树内 HTTP RPC + RocksDB raft CF；M1–M3 里程碑；2026-08-06）
- [x] 多节点数据面实施（**M1–M3 + P4-01–P4-04 完成**（2026-08-08：M1 单节点 openraft 适配，cluster 17→21 测；M2 多进程 HTTP RPC + RocksDB raft CF + snapshot install，cluster 21→26 测；M3 默认运行时切换 + CI 三进程 smoke + 真实复制语义，cluster 26→27 测；P4-01 多进程元数据 RPC，cluster 27→28 测；P4-03 跨节点数据搬迁经 `/internal/raft/transfer-snapshot` + `DataPlaneSnapshotIo` 真实字节迁移 + P4-04 真实网络分区经 `HttpRaftClient` 对称丢弃/隔离拒绝/愈合恢复，cluster 28→30 测））
- [x] 完整 W3C 套件接入（vendored `w3c/rdf-tests` sparql11 941 文件/28 feature + manifest 驱动 `w3c11_suite` runner + `w3c11_profile.tsv` 492 条基线：492 PASS / 0 FAIL（fail=0、drift=0 全绿）；2026-08-06 接入、2026-08-07 第七波 339→434、2026-08-08 第八波 434→444、第九波 444→458、第十波 458→492）
- [x] 完整聚合（GROUP BY/HAVING + COUNT(DISTINCT)/SUM/AVG/MIN/MAX + 子查询聚合，W3C must-pass 27/27）
- [x] SPARQL Update 基线（INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE，W3C must-pass 30/30、skip=0）
- [x] 在线重平衡与灾备演练手册及证据（P7-01 / P7-04）：`drill-rebalance-dr.sh` 真实 3 进程 raft DRILL PASS + [L7-ops-rebalance-dr.md](./L7-ops-rebalance-dr.md)（2026-08-08）
- [x] 发布流水线与回滚演练通过（P7-03）：[L7-release-rollback.md](./L7-release-rollback.md) 代码级/数据级回滚流程与验证判据 + **实际演练 DRILL PASS**（2026-08-08：[`scripts/release-rollback-drill.sh`](../scripts/release-rollback-drill.sh) staging 全流程，`/health triples` 1→1→0→1、二进制指纹 2→0→2）
- [x] R1 退出标准全表勾选（2026-08-08）：SPARQL 查询基线 / 单区域集群核心 / 标准符合性门禁 / 核心 SLO 基线 / 恢复演练 / **实际发布回滚演练**（`release-rollback-drill.sh` staging DRILL PASS）/ **RDF 核心运行时可验收**（2026-08-08：[R1-acceptance-package.md](./R1-acceptance-package.md) 正式验收包，G1–G5 全 PASS）/ **OIDC 完整链路（安全基线，R2+ 轨）**（2026-08-08：[L5-ontolith-access-security.md](./L5-ontolith-access-security.md) §2 OIDC 小节，security 18→24、server 44→49 测）——全表勾选完成

### R1 关键路径（按依赖序）

1. [~] Phase 0 签批与模板（模板齐；签批未做）
2. [x] Phase 1 KO 模型 + 存储契约文档（SAS-0401 + L2-storage-contracts）
3. [x] Phase 2 RocksDB + 多索引 + 事务文档（L2-storage-transaction-kernel + 磁盘 MVCC）
4. [x] Phase 3 真 SPARQL MVP（含完整聚合）+ Explain/超时 + R1 烟雾（W3C 492/492 全绿）
5. [x] Phase 4 单区域集群最小闭环（**M1–M3 + P4-01–P4-04 已落地**：单节点 raft 数据面 + 双/三节点 HTTP+RocksDB 复制 + 默认运行时切换 + CI 三进程 smoke + 多进程元数据 RPC + 跨节点数据搬迁（`/internal/raft/transfer-snapshot` + `DataPlaneSnapshotIo`）+ 真实网络分区（客户端对称丢弃 + 隔离拒绝/愈合恢复）+ 在线重平衡/灾备演练）
6. [x] Phase 5 网关 + 鉴权/租户/审计落盘（Tracing 全链路 + OIDC-ready JWT 已落地）
7. [x] R1 退出标准全表勾选（2026-08-08；实际发布回滚演练 + 正式验收包 + OIDC 完整链路 R2+ 均已完成）

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
- [L7 在线重平衡与灾备演练手册](./L7-ops-rebalance-dr.md) · [L7 发布/回滚手册](./L7-release-rollback.md)
- [R1 SPARQL 烟雾符合性](./R1-sparql-smoke-compliance.md) · [R1 正式验收包](./R1-acceptance-package.md)
- [CI workflow](../.github/workflows/ci.yml) · [ci-local](../scripts/ci-local.sh)
- [GitHub Projects #2 同步契约](./github-projects-sync.md)（SYNC-PROJ-0001）
- [ADR 模板](../adr/0000-template.md) · [RFC 模板](../rfc/0000-template.md)
- [RFC-0001 确定性标识与规范化编码规则](../rfc/0001-canonical-encoding-and-disk-layout.md)
