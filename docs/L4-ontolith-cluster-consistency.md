# L4 — Cluster & Consistency

文档 ID: IMPL-L4-0001  
版本: 2.7.0  
状态: Implemented (in-process simulator harness + multi-process raft M3 + P4-01–P4-04 via ADR-0004)  
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
| 多进程 openraft | ADR-0004 | **M1–M3 + P4-01–P4-04 已落地**（2026-08-08：M1 openraft 0.9.25 单节点引导 + trait 适配 + 内存传输，cluster 17→21 测；M2 多进程 HTTP RPC + RocksDB `raft` CF + snapshot install，cluster 21→26 测；M3 默认运行时切换 + 真实复制语义 + CI 三进程 smoke，cluster 26→27 测；P4-01 多进程元数据 RPC——`/internal/raft/apply` + 复制式节点注册表，cluster 27→28 测；P4-03 跨节点数据搬迁——`DataPlaneSnapshotIo` + `/internal/raft/transfer-snapshot` 真实字节迁移，cluster 28→29 测；P4-04 真实网络分区——`HttpRaftClient` 对称丢弃 + 隔离拒绝/愈合恢复，cluster 29→30 测） |

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
| ontolith-cluster | **30**（+session/partition/commit/rebalance + data-plane sync + raft M1：单节点引导/append-commit/trait 适配/双节点内存传输 + raft M2：RocksDB 日志/快照往返、HTTP 共享 secret 认证、HTTP install-snapshot RPC 往返、双节点 HTTP+RocksDB 选举/复制/落盘 + raft M3：三节点 HTTP+RocksDB 多数派提交/失一 follower 后仍可提交 + P4-01：三节点元数据 register/heartbeat/status 跨节点复制收敛 + P4-03：快照字节经 HTTP 迁移到目标并记录导入调用/无 hook 回退不触网 + P4-04：隔离 leader+follower 后元数据转发确定性失败/heal 后选主与转发恢复） |
| ontolith-server | **9**（+`/cluster` API） |

---

## 6. 边界

1. 单进程内存控制面，非生产多机 HA  
2. Rebalance 已配套数据面搬迁通道（`DataPlaneSync`：快照传输入队/完成/回执）；多进程 RPC 传输已实现（M2 树内 HTTP/1.1，`/internal/raft/*` + 共享 secret），P4-03 起数据搬迁经 `/internal/raft/transfer-snapshot` + `DataPlaneSnapshotIo` 真实字节迁移（调用节点导出、目标节点导入，无 hook 回退模拟回执）  
3. 数据面仍由各节点本地 L2 引擎负责  
4. 生产 Raft 决策已定：[ADR-0004](../adr/0004-multi-process-raft-data-plane.md)（openraft behind traits，M1–M3 + P4-01–P4-04 已落地）；M3 起管理二进制默认运行时切换为 `RaftClusterRuntime`（`ONTOLITH_CLUSTER_MODE` 默认 `raft`，`memory` 显式选择模拟器 harness），`InMemoryClusterRuntime` 降级为测试/CI 确定性 harness；P4-01 起元数据变异（register/heartbeat/set_node_status）经 raft 提交并复制到全节点；P4-04 起 `FaultInjector` 为真实对称网络分区（`HttpRaftClient` 按 `partition` 集合丢弃 RPC，`metadata_mutation` 拒绝隔离节点与转发到隔离 leader，heal 后恢复）  

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
| 2026-08-08 | 2.5.0 | **M3 落地**：默认运行时切换 + CI 三进程 smoke（ADR-0004 决策 5）。`AppState.cluster` 改 `Arc<dyn ClusterRuntime>`；管理二进制 `ONTOLITH_CLUSTER_MODE` 默认 `raft`（内存模拟器降级 harness，`memory` 显式选择），raft 参数 `ONTOLITH_RAFT_NODE_ID/LISTEN/SECRET/MEMBERS/STORAGE_PATH`；多节点同成员 `initialize` 容忍 `NotAllowed`；`replicate_to_followers` 真实语义（leader replication metrics 水印增量）+ `applied_index` follower acked index；`/admin/data/replicate?append=1` 驱动 raft 写入；cluster 26→27 测（三节点多数派提交/失一 follower 后仍可提交）+ CI multi-node raft smoke（3 进程） |
| 2026-08-08 | 2.6.0 | **P4-01 落地**：多进程元数据服务与主从选举收尾——`LogPayload` 增 `RegisterNode`/`Heartbeat`/`SetNodeStatus` 变体（`ClusterNode`/`NodeRole`/`NodeStatus` 等增 serde）；`RaftClusterRuntime` 复制式节点注册表（`nodes` + `applied_watermark` 增量折叠，RocksDB 重启重建）；元数据变异经 `metadata_mutation` 提交（leader 本地 `client_write`，follower 经 `/internal/raft/apply` 转发、409 携带 leader 提示、重试 ≤3）；`membership()`/`status()` 读复制式注册表（role 按 leader 刷新）；cluster 27→28 测 |

| 2026-08-08 | 2.7.0 | **P4-03/P4-04 落地**：`DataPlaneSync for RaftClusterRuntime` 真实实现——`DataPlaneSnapshotIo` trait（`export_snapshot`/`import_snapshot`）+ `/internal/raft/transfer-snapshot` RPC，`complete_transfer` 导出字节 POST 至目标导入、200 返回 `SyncReceipt`，无 hook 回退模拟回执；`FaultInjector` 真实对称网络分区——`HttpRaftClient::post` 按 `partition` 集合丢弃 RPC（target/self 命中即 `RPCError::Network`），`metadata_mutation` 拒绝隔离节点自身与转发到隔离 leader，`complete_transfer` 拒绝迁移到分区目标，heal 后恢复；cluster 28→30 测 |