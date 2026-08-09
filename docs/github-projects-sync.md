# GitHub Projects 同步契约（Projects v2 → #2）

文档 ID: SYNC-PROJ-0001
目标看板: <https://github.com/users/shark8848/projects/2>（用户级 Projects v2）
数据源: [PROGRESS.md](./PROGRESS.md)（单一事实源，版本 0.1.40，2026-08-09）
状态: Active（Classic PAT 已配置，随增量同步）

## 1. 认证要求（重要，勿重复探索）

- GitHub **用户级** Projects v2 **不支持 fine-grained PAT**（实测 2026-08-08：
  `viewer{projectsV2}` 与 `user(login){projectsV2}` 均返回
  `FORBIDDEN: Resource not accessible by personal access token`，
  REST `/users/{login}/projects` 返回 404）。
- 可用令牌（二选一）：
  1. **Classic PAT**（`https://github.com/settings/tokens` → Generate new token
     (classic)），勾选 `project` scope（读写 Projects v2；仅读可勾
     `read:project`）。格式以 `ghp_` 开头。
  2. **GitHub App 安装令牌**，权限 `Projects: Read and write`（用户项目）。
- 令牌放 `/tmp/gh_token`（chmod 600），勿入库。
- 组织级 Projects 才支持 fine-grained PAT（org 级 `Projects` 权限）；本项目为
  用户级，不在其列。

## 2. 读取看板（首次同步先执行）

```bash
TOKEN=$(cat /tmp/gh_token)
# 1) 项目 node ID（number=2）
curl -sS -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  https://api.github.com/graphql -d '{"query":"query{user(login:\"shark8848\"){projectV2(number:2){id title url}}}"}'
# 2) 字段结构（Status 列名/选项，用上一步返回的 PROJECT_ID）
#    node(id:"PROJECT_ID"){fields(first:50){nodes{... on ProjectV2SingleSelectField{name options{name}}}}}
# 3) 现有条目
#    node(id:"PROJECT_ID"){items(first:100){nodes{id title fieldValues(first:10){nodes{... on ProjectV2ItemFieldSingleSelectValue{name field{... on ProjectV2SingleSelectField{name}}}}}}}}
```

字段名与选项以看板实际为准；本契约的映射表按 PROGRESS.md 维护，标题/状态
不一致时以 PROGRESS.md 为准并回写看板。

## 3. 条目映射（2026-08-08 快照，随 PROGRESS.md 更新）

状态取值：`未开始` / `进行中` / `已完成`。完成度百分比进备注。

| 看板条目标题 | 状态 | 备注 |
|--------------|------|------|
| P0-01 已批准范围基线签批 | 未开始 | PLAN-0001 仍 Draft，阻塞项 |
| P0-02 架构例外审批模板 | 已完成 | adr/0000-template + ADR-0001/0002 |
| P0-03 依赖登记模板与评审规则 | 进行中 | 70%，DEPENDENCY_REGISTER.md |
| P0-04 RFC 流程落地 | 进行中 | 70%，RFC-0001 评审待回填 |
| P0-05 进度台账 | 已完成 | PROG-0001 |
| P1-01 Knowledge Object 领域模型 | 进行中 | 80%，L0+L1+序列化 Part II |
| P1-02 Node 标识与字典管理器 | 进行中 | 90% |
| P1-03 存储抽象接口 | 进行中 | 95%，接口版本 0.1.0 冻结 |
| P1-04 确定性标识与规范化编码规则 | 进行中 | 95%，RFC-0001 |
| P2-01 RocksDB 适配 | 进行中 | 90% |
| P2-02 WAL/快照恢复/MVCC 基线 | 已完成 | 内存+磁盘 MVCC 版本链 |
| P2-03 三元组/四元组物理编码 | 进行中 | 90% |
| P2-04 索引基线 SPO/POS/OSP | 已完成 | +命名图 GSPO/GPOS/GOSP |
| P2-05 可恢复耐久写入路径 | 已完成 | sync_writes + BackupEngine 演练 |
| P2-06 事务行为规范文档 | 进行中 | 95%，L2 文档 v3 |
| P3-01 SPARQL 核心代数/优化/绑定 | 已完成 | 含属性路径最小集 |
| P3-02 完整聚合 | 已完成 | GROUP BY/HAVING + 聚合函数 |
| P3-03 SPARQL Update 基线 | 已完成 | INSERT/DELETE DATA、DELETE·INSERT、DELETE WHERE |
| P3-04 W3C 套件合规 492/492 | 已完成 | fail=0、drift=0 |
| P4-01 多进程元数据 RPC | 已完成 | cluster 27→28 测 |
| P4-02 多进程 Raft 数据面（M1–M3） | 已完成 | openraft + HTTP RPC + RocksDB raft CF |
| P4-03 跨节点数据搬迁 | 已完成 | transfer-snapshot + DataPlaneSnapshotIo |
| P4-04 真实网络分区演练 | 已完成 | 对称丢弃/隔离拒绝/愈合恢复 |
| P5-01 gRPC 网关 | 已完成 | tonic + metadata 鉴权 |
| P5-02 OIDC/JWT 基线 | 已完成 | HS256 Bearer |
| OIDC 完整链路 R2+ | 已完成 | 2026-08-08，JWKS/RS256/发现文档/TTL 缓存 |
| P5-03 强制租户隔离 | 已完成 | TenantNamespace + 越权 403 |
| P5-05 Tracing 全链路 | 已完成 | traceparent 延续 + /admin/traces |
| 管理面 TLS 终止 + R2 门禁 | 已完成 | rustls + 非 loopback 强制 |
| P6-01 规则扩展 | 已完成 | 前向链推理引擎 |
| P6-02 SHACL 核心组件 + W3C 套件 | 已完成 | 98/98 全绿（2026-08-09 闭合 uniqueLang-002） |
| P6-03 server 查询/推理管线 | 已完成 | |
| P7-01 在线重平衡演练手册 | 已完成 | drill-rebalance-dr.sh DRILL PASS |
| P7-02 阈值断言/趋势记录 | 已完成 | storage_bench + check-bench-thresholds.sh |
| P7-03 发布/回滚手册 + 实际演练 | 已完成 | release-rollback-drill.sh DRILL PASS 2026-08-08 |
| P7-04 灾备运维手册 | 已完成 | L7-ops-rebalance-dr.md |
| 首次真实发布（生产） | 未开始 | P7-03 手册就绪，待首次发布 |
| L8 AI-Native 扩展 | 进行中 | Phase 8 整体 ~15%（2026-08-09：P8-01 M1+M2+M3） |
| P8-01 语义-向量桥接 | 进行中 | 80%（M3 持久化 + 增量更新完成，2026-08-09） |
| P8-01 M3 语义索引持久化 + 增量更新 | 已完成 | RocksDB `semantic` CF + `RocksSemanticIndex` + 删改回流，2026-08-09 |
| P8-02 检索增强接口 | 进行中 | 30%（HTTP 接口落地，KPI 门禁待收尾） |
| P8-03 代理集成扩展点 | 未开始 | 依赖 P8-01/P8-02 |
| R1 退出标准全表 | 已完成 | 2026-08-08 全勾选（含正式验收包 G1–G5 全 PASS） |
| R2 查询代价模型与高级 Update | 进行中 | W3C 剩余欠账 + 代价模型 |
| R2 OWL 2 RL 推理 | 未开始 | |
| R2 Explain 门禁 | 未开始 | |
| R2 推理护栏 | 未开始 | |
| R2 SHACL 97/98 缺口收尾 | 已完成 | 2026-08-09 闭合，98/98 全绿 |
| R2 退出标准全表勾选 | 已完成 | 2026-08-08 R2 全项（Explain 门禁 + 推理护栏） |
| RDF 1.1 布尔项区分（uniqueLang-002 闭合） | 已完成 | 2026-08-09 |

## 4. 写入操作（GraphQL）

```bash
TOKEN=$(cat /tmp/gh_token); PROJECT_ID="..."; FIELD_ID="..."; STATUS_DONE="已完成"
# 新增条目：projectV2AddProjectV2Item
#  curl -sS -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
#    https://api.github.com/graphql -d '{"query":"mutation($p:ID!,$t:String!){projectV2AddProjectV2Item(input:{projectId:$p,contentId:$t}){item{id}}}","variables":{"p":"PROJECT_ID","t":"REPO_ISSUE_OR_DRAFT_ID"}}'
# 更新状态字段：updateProjectV2ItemFieldValue（singleSelectOptionId）
# 删除条目：deleteProjectV2Item
```

条目 content 可挂仓库 issue（`shark8848/ontolith`）或看板草稿；纯进度跟踪建议
用草稿 + Status 字段，保持看板自包含。

## 5. 同步流程

1. 维护条目 TSV（标题 / 状态 / 优先级，格式见脚本头注释）。
2. 执行入库脚本（幂等 upsert：按标题匹配，缺失创建、存在只更新字段）：
   ```bash
   bash scripts/sync-github-projects.sh /path/to/items.tsv
   ```
   前置：Classic PAT（`project` scope）写入 `/tmp/gh_token`（chmod 600）。
3. 回填 PROGRESS.md 变更记录：`已同步到 GitHub Projects #2（SYNC-PROJ-0001）`。
4. commit + push 同步记录（origin 推送契约见 AGENTS.md）。
