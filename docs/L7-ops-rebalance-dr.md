# L7 — 在线重平衡与灾备演练手册

文档 ID: OPS-L7-0001  
版本: 1.0.0  
状态: Implemented（P7-01 在线重平衡 + P7-04 灾备演练）  
日期: 2026-08-08  
对应脚本: `scripts/drill-rebalance-dr.sh`  
对应代码: `crates/ontolith-cluster/src/infrastructure/{mod,raft}/mod.rs`、`crates/ontolith-server/src/{app,management}.rs`

---

## 1. 目标

用**真实多进程 raft 集群**（openraft HTTP RPC + RocksDB `raft` CF，与 CI 三进程 smoke
同路径）演练两条运维主线：

- **P7-01 在线重平衡**：控制面把槽位映射从“刻意偏斜”的初始状态在线重划为均衡，
  过程无需停机。
- **P7-04 灾备演练**：多数派存活下的 follower 失联、自动 failover 下的 leader 失联、
  以及节点重启后基于原存储路径追赶提交，全程可复现、可留证据。

演练脚本是一个**硬断言门禁**：任一环节失败即以非零退出码结束，并留下完整证据包。

## 2. 前置条件

```bash
# 构建含 raft 数据面 + slot bias 的管理服务器（演练依赖该二进制）
cargo build -p ontolith-server --bin ontolith-management-server
```

默认使用 `target/debug/ontolith-management-server`，可用
`ONTOLITH_DRILL_BIN` 覆盖。需要本机可用 RocksDB（默认 feature 已含）。

## 3. 运行

```bash
bash scripts/drill-rebalance-dr.sh
```

成功时末尾输出 `=== DRILL PASS ===`，退出码 0；失败时退出码非 0 并打印失败步骤。

### 可调参数（环境变量）

| 变量 | 默认 | 说明 |
|------|------|------|
| `ONTOLITH_DRILL_BIN` | `target/debug/ontolith-management-server` | 演练二进制 |
| `ONTOLITH_DRILL_NODES` | `3` | 节点数（须 ≥2；leader 杀除演练需要 ≥3） |
| `ONTOLITH_DRILL_BASE_PORT` | `28000 + RANDOM%2000` | raft/管理面端口基址（每节点 +1、管理面 +100） |
| `ONTOLITH_DRILL_SLOT_BIAS` | `256` | 初始槽位偏斜量（演练在线重平衡必须有真计划） |
| `ONTOLITH_DRILL_EVIDENCE_DIR` | `$TMPDIR/ontolith-drill-*/evidence` | 证据输出目录 |
| `ONTOLITH_DRILL_ELECTION_WAIT` | `90` | 选主等待秒数 |
| `ONTOLITH_DRILL_CONVERGE_WAIT` | `90` | 复制收敛等待秒数 |
| `ONTOLITH_DRILL_REJOIN_WAIT` | `120` | 重启追赶等待秒数 |

## 4. 演练步骤与判据

| # | 步骤 | 判据 |
|---|------|------|
| 1 | 选主 | 某节点成为 leader（`/admin/monitoring` 出现 `leader:n[0-9]+`） |
| 2 | 在线重平衡 | `POST /admin/data/rebalance` 返回 `plans>0` 且 `shard_map_epoch` 前进 |
| 3 | 复制基线 | 追加 3 条日志后全部节点 `commit_index` 收敛 |
| 4 | 杀 follower | 多数派存活下 commit 继续前进（`commit2 > commit`） |
| 5 | 重启 follower | 重启后基于原存储路径追赶至 `commit2` |
| 6 | 杀 leader | 自动 failover：新 leader 选出且 commit 继续前进（`commit3 > commit2`） |
| 7 | 重启原 leader | 基于原存储路径追赶至 `commit3`，集群回到 3 节点全绿 |

> 顺序关键：杀 leader 前必须先恢复 follower 回到 3 节点——只剩 2 节点时杀 leader
> 将无多数派，属 raft 正确行为而非故障。脚本按此顺序编排。

## 5. 证据包

默认输出到 `$TMPDIR/ontolith-drill-<pid>/evidence/`（脚本末尾打印路径）：

- `drill-transcript.txt`：带时间戳的逐步日志（选主、rebalance 计划数、shard_map_epoch
  前后值、各阶段 commit 推进、重启追赶结果）。
- `logs/nodeN.log`：每个节点的完整运行日志（异常时用于排查）。

证据示例（关键行）：

```
[02:11:05] elected leader: n1 (admin http://127.0.0.1:28214)
[02:11:05] rebalance: {"plans":2,"epoch":0,"shards":2,"shard_map_epoch":2}
[02:11:05] OK: online rebalance moved 2 slot ranges, shard-map epoch 0 -> 2
[02:11:07] OK: majority commit survived follower loss (5 -> 6)
[02:11:09] OK: automatic failover: commit advanced 6 -> 7 under n0
[02:11:10] OK: node n1 rejoined and caught up to commit 7
[02:11:10] === DRILL PASS ===
```

`shard_map_epoch` 是重平衡生效的直接证据（raft 侧 `epoch` 为 openraft term，不随
重平衡变化）。

## 6. 与实现的对应关系

- `ClusterConfig.initial_slot_bias` / `apply_initial_slot_bias`：启动时把槽位边界右移
  `bias` 个槽（末分片收缩），使首次 `rebalance()` 产生真计划
  （`crates/ontolith-cluster/src/infrastructure/mod.rs`）。
- `RaftClusterConfig.initial_slot_bias`：透传到控制面 shard map
  （`crates/ontolith-cluster/src/infrastructure/raft/mod.rs`）。
- `ONTOLITH_RAFT_SLOT_BIAS`：管理服务器环境变量入口
  （`crates/ontolith-server/src/app.rs`）。
- `/admin/monitoring` 与 `/admin/data/rebalance` 新增 `shard_map_epoch` 字段
  （`crates/ontolith-server/src/management.rs`）。
- 单测 `rebalance_moves_slots_after_initial_bias`：偏斜 → 重平衡 → 均衡的进程内断言。

## 7. CI 归属

CI 的 `check` 作业已内置三进程 raft smoke（选主/复制/follower 失联多数派提交）。
本演练是它的超集（含在线重平衡 + leader failover + 双节点重启追赶），按需在
发布前或定期灾备日手动执行，证据包归档至运维记录。
