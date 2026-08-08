# L4 — Cluster & Consistency

文档 ID: IMPL-L4-0001  
版本: 2.4.0  
状态: Implemented (in-process + multi-process raft M2 via ADR-0004)  
日期: 2026-08-08  
对应 crate: `crates/ontolith-cluster`（+ L5 `/cluster/*`）

---

## 1. 能力矩阵

| 能力 | v1 | v2（本轮） |
|------|----|------------|
| 元数据 / 选主 / 分片 / 复制 / failover | ✅ | ✅ |
| Consistency 读路由 Strong/Eventual | ✅ | ✅ |
| Session 粘性读 | — | ✅ `route_read_session` |
| 多数派 commit_index | — | ✅ |
| 网络分区注入 / 愈合 | — | ✅ 少数派不可选主 |
| 在线 rebalance（slot 重划） | — | ✅ |
| ClusterStatus 汇总 | — | ✅ |
| L5 HTTP `/cluster/*` | — | ✅ |
| 多进程 openraft | ADR-0004 | **M1–M2 已落地**（2026-08-08：M1 openraft 0.9.25 单节点引导 + trait 适配 + 内存传输，cluster 17→21 测；M2 多进程 HTTP RPC + RocksDB `raft` CF + snapshot install，cluster 21→26 测，双节点 HTTP+RocksDB 选举/复制/落盘）；M3 默认运行时切换 + CI 三进程 smoke |

---

## 2. 运行时 API（Rust）

```rust
let rt = InMemoryClusterRuntime::with_defaults();
rt.bootstrap(vec![("n1", "…"), ("n2", "…"), ("n3", "…")])?;

// Session sticky
rt.route_read_session("k", &SessionId::new("s1"), ConsistencyLevel::Session)?;

// Quorum commit after replicate
rt.append(LogPayload::Metadata("x".into()))?;
rt.replicate_to_followers()?;
assert_eq!(rt.commit_index(), rt.leader_index());

// Chaos
rt.inject_partition(vec![ClusterNodeId::new("n1"), ClusterNodeId::new("n2")])?;
assert!(rt.campaign(&ClusterNodeId::new("n3"))?.is_none()); // minority
rt.heal_partition()?;

// Rebalance slots
let plans = rt.rebalance()?;
```

### 契约扩展

| Trait | 新增 |
|-------|------|
| `MetadataService` | `status() -> ClusterStatus` |
| `ShardRouter` | `route_read_session` |
| `Replicator` | `commit_index`, `replicate_to_followers_respecting_partition` |
| `RebalanceService` | `rebalance` / `rebalance_history` |
| `FaultInjector` | `inject_partition` / `heal_partition` / `current_partition` |

---

## 3. L5 HTTP 面

| Method | Path | 说明 |
|--------|------|------|
| GET | `/cluster` `/cluster/status` | epoch/leader/nodes/commit/partition |
| GET | `/cluster/membership` | 节点列表 |
| GET | `/cluster/shards` | slot 分配与 replica |
| GET | `/cluster/route?key=&consistency=&session=` | 读写路由 |
| POST | `/cluster/heartbeat?node=&tick=` | 心跳 |
| POST | `/cluster/tick?tick=` | 推进时钟并 failover |
| POST | `/cluster/replicate?append=1` | 追平 follower（可选 append） |
| POST | `/cluster/rebalance` | 均匀重划 slots |
| POST | `/cluster/partition?nodes=n1,n2` | 注入分区 |
| POST | `/cluster/heal` | 愈合分区 |
| GET | `/cluster/failover` | 故障转移历史 |

权限：与 `/health` 相同（`health:read`）。

---

## 4. 一致性与分区语义

| 级别 | 行为 |
|------|------|
| Strong | 始终 leader |
| Session | 粘性到上次节点；失效则回 leader |
| Eventual | 优先 lag 可接受的 follower |

分区：

- 被隔离节点不参与投票/复制  
- **选主需要全体 votable 的多数**（防脑裂）  
- `commit_index` 仅统计可达 voter 的 applied 多数  

---

## 5. 测试

| Crate | 数量 |
|-------|------|
| ontolith-cluster | **26**（+session/partition/commit/rebalance + data-plane sync + raft M1：单节点引导/append-commit/trait 适配/双节点内存传输 + raft M2：RocksDB 日志/快照往返、HTTP 共享 secret 认证、HTTP install-snapshot RPC 往返、双节点 HTTP+RocksDB 选举/复制/落盘） |
| ontolith-server | **9**（+`/cluster` API） |

---

## 6. 边界

1. 单进程内存控制面，非生产多机 HA  
2. Rebalance 已配套数据面搬迁通道（`DataPlaneSync`：快照传输入队/完成/回执）；多进程 RPC 传输已实现（M2 树内 HTTP/1.1，`/internal/raft/*` + 共享 secret），真实网络分区注入仍缺（M3+）  
3. 数据面仍由各节点本地 L2 引擎负责  
4. 生产 Raft 决策已定：[ADR-0004](../adr/0004-multi-process-raft-data-plane.md)（openraft behind traits，M1–M2 已落地，M3 默认运行时切换进行中）；M3 完成前默认运行时仍为 ADR-0002 模拟器（`RaftClusterRuntime` 需显式启用 HTTP+RocksDB 配置）  

---

## 7. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | 单区域 MVP 闭环 |
| 2026-07-17 | 2.0.0 | session 粘性、quorum commit、partition、rebalance、L5 `/cluster` |
| 2026-08-06 | 2.1.0 | 数据面同步（`DataPlaneSync`）：快照式 slot 迁移入队/`drain_syncs` 完成/`SyncReceipt` 回执，含源目标节点校验；`rebalance` 计划可经数据面搬迁，+3 测 |
| 2026-08-06 | 2.2.0 | 多进程 Raft 设计定稿（[ADR-0004](../adr/0004-multi-process-raft-data-plane.md)）：openraft 共识引擎 + 树内 axum/reqwest HTTP RPC（`/internal/raft/*`，共享 secret 认证）+ RocksDB `raft` CF 日志/快照 + 写入路径经 raft 多数派提交后落 L2；保留 `InMemoryClusterRuntime` 作为确定性测试 harness |
| 2026-08-08 | 2.3.0 | **M1 落地**：openraft 0.9.25 单节点引导 + 内存存储（v1 `RaftStorage` + `Adaptor`）+ 内存 `RaftNetworkFactory` 传输（`RaftRegistry`）+ `RaftClusterRuntime` 集群 trait 适配器（选主/epoch/复制日志/commit 由 raft 背书）；cluster 17→21 测 |
| 2026-08-08 | 2.4.0 | **M2 落地**：多进程 HTTP RPC + RocksDB `raft` CF + snapshot install（ADR-0004 决策 2/3）。openraft `serde` feature + `serde_json`；树内最小 HTTP/1.1 RPC（`HttpRaftServer`/`HttpRaftClient`/`HttpRaftFactory`，`/internal/raft/{vote,append-entries,install-snapshot}`，`Authorization: Bearer <secret>` 共享 secret 认证，raft 错误 serde 往返为 `RemoteError`；沿用 L5 同风格最小 HTTP 栈，未引入 axum/reqwest）；`RocksDbStorageEngine` 增独立 `raft` CF 字节级原语（`raft_cf_*` + `RaftCfOp` 原子 batch），`RocksRaftStorage`（v1 `RaftStorage` + `RocksLogReader`/`RocksSnapshotBuilder`：日志/vote/committed/purged/membership/应用态/快照 meta+字节，snapshot build/install 原子替换）；`RaftClusterConfig` 增 `http_listen_addr`/`raft_secret`/`raft_storage_path`，运行时配置化选择存储与传输（内存回退保留）；cluster 21→26 测 |
