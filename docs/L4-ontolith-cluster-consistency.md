# L4 — Cluster & Consistency

文档 ID: IMPL-L4-0001  
版本: 2.0.0  
状态: Implemented (single-region in-process + L5 HTTP surface)  
日期: 2026-07-17  
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
| 多进程 openraft | 延期 | 延期（ADR-0002） |

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
| ontolith-cluster | **14**（+session/partition/commit/rebalance） |
| ontolith-server | **9**（+`/cluster` API） |

---

## 6. 边界

1. 单进程内存控制面，非生产多机 HA  
2. Rebalance 只改 slot 映射，不做数据搬迁  
3. 数据面仍由各节点本地 L2 引擎负责  
4. 生产 Raft 需新 ADR（openraft）  

---

## 7. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | 单区域 MVP 闭环 |
| 2026-07-17 | 2.0.0 | session 粘性、quorum commit、partition、rebalance、L5 `/cluster` |
