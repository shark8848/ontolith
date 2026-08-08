# L7 — 发布流水线与回滚手册

文档 ID: OPS-L7-0002  
版本: 1.0.0  
状态: Implemented（P7-03 发布流水线与回滚验证）  
日期: 2026-08-08  
对应: [.github/workflows/ci.yml](../.github/workflows/ci.yml)、`scripts/ci-local.sh`、
`deployments/`、`scripts/install-ontolith*.sh`

---

## 1. 发布流水线

### 1.1 门禁链（GitHub Actions，`.github/workflows/ci.yml`）

| 作业 | 内容 | 性质 |
|------|------|------|
| `check` | fmt / clippy `-D warnings` / workspace 全量测试 / 管理面 smoke + SLO 短窗 / SPARQL R1 / 三进程 raft smoke | 硬门禁 |
| `w3c-subset` | SPARQL W3C must-pass 30/30（required-lite） | 硬门禁 |
| `w3c-subset-strict` | 全量 W3C 观测轨（non-blocking） | 观测 |
| `w3c-subset-strict-readiness` | 连续 3 次 main 全绿 → “ready” 信号 | 观测 |
| `rocksdb-smoke` | RocksDB 后端 reopen / 后端路径测试 | 硬门禁 |
| `bench` | 存储微基准 + **阈值断言** + 趋势记录（P7-02） | 硬门禁 |
| `license-audit` | `cargo-license` 第三方许可枚举 | 硬门禁 |

本地等效：`bash scripts/ci-local.sh`（不依赖网络/Docker，覆盖 fmt/clippy/测试/
R1 smoke/W3C 子集/管理面 smoke/SLO 短窗）。

### 1.2 发布步骤

```bash
# 1) 本地全量验证（等价 CI check 核心）
bash scripts/ci-local.sh

# 2) 基准回归（可选但建议）
ONTOLITH_BENCH_TREND_PATH=benchmarks/trends/storage-bench.jsonl \
  bash scripts/check-bench-thresholds.sh

# 3) 灾备演练（发布前必做，P7-01/04）
cargo build -p ontolith-server --bin ontolith-management-server
bash scripts/drill-rebalance-dr.sh

# 4) 构建 release 二进制
cargo build --release -p ontolith-server --bin ontolith-server
cargo build --release -p ontolith-server --bin ontolith-management-server

# 5) 部署
./scripts/install-ontolith-service.sh                 # runtime server（system）
./scripts/install-ontolith-management-system-service.sh  # 管理面（system）
# 或 user systemd（无 sudo）：
./scripts/install-ontolith-user-service.sh
./scripts/install-ontolith-management-user-service.sh

# 6) SLO 采集/评估 timers（一次性）
./scripts/install-ontolith-slo-timers.sh
```

非 loopback 暴露的管理面必须配 TLS（`ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY`，
R2 门禁：非 loopback bind 无 TLS 拒绝启动；自签证书见
`scripts/gen-self-signed-cert.sh`）。

## 2. 发布后验证

```bash
curl -fsS http://127.0.0.1:8090/health
curl -fsS http://127.0.0.1:19091/admin/health          # 管理面
curl -fsS http://127.0.0.1:19091/admin/monitoring      # runtime probe + cluster
systemctl --user status ontolith-server ontolith-management-server  # user 模式
```

SLO 门禁自检：

```bash
bash scripts/check-slo-window-history.sh --self-test   # 窗口 SLO（健康窗/连续失败/P95/尖峰）
bash scripts/check-management-slo-window.sh            # 短窗 SLO 检查
```

## 3. 回滚验证

回滚原则：**二进制与数据分离**。数据目录（`/var/lib/ontolith/data`、
`ONTOLITH_RAFT_STORAGE_PATH` 指向的 raft 目录）不回退；只回退可执行文件与配置，
保持磁盘格式向前兼容（L2 旧库打开自动回填、raft CF 原路径复用）。

### 3.1 代码级回滚（回退上一个已发布版本）

```bash
# 1) 找到上一个发布 tag/commit
git tag -l 'v*'   # 或 git log --oneline -5
git checkout <prev-release-tag>   # 示例：git checkout v0.1.29

# 2) 重新构建
cargo build --release -p ontolith-server --bin ontolith-server
cargo build --release -p ontolith-server --bin ontolith-management-server

# 3) 重跑 CI 门禁关键项
bash scripts/ci-local.sh

# 4) 重部署（复用 §1.2 第 5 步脚本；数据目录不动）
./scripts/install-ontolith-service.sh
./scripts/install-ontolith-management-system-service.sh
```

systemd 会把新二进制装上并 `restart`；环境文件（`/etc/ontolith/ontolith.env`、
`~/.config/ontolith/ontolith.env`）如已被新版本改动，回滚时恢复为 tag 内的
`deployments/*.env`。

### 3.2 数据级回滚（异常数据损坏，谨慎操作）

L2 提供 RocksDB BackupEngine 备份/恢复原语（`create_backup` / `restore_backup`，
storage 单测覆盖 `restore_backup` 演练）。操作前必须停服：

```bash
systemctl stop ontolith-server
# 备份当前数据（如可启动）后，用最近一次已知良好备份覆盖数据目录
# 恢复完成后：
systemctl start ontolith-server
# 验证：
curl -fsS http://127.0.0.1:8090/health
# 集群模式：确认全部节点从各自备份/快照恢复后 commit_index 收敛
```

> 备份调度/保留策略接入管理面为运维后续轨（P2-05 备注）；回滚演练推荐在
> 灾备日与 `drill-rebalance-dr.sh` 一并执行。

### 3.3 回滚验证判据

- [ ] 新二进制部署后 `/health` 与 `/admin/health` 返回 200
- [ ] `runtime_probe.reachable=true` 且 `latency_ms` 低于 SLO 阈值
- [ ] 全量测试门禁通过（`ci-local.sh`）
- [ ] 数据面无回退：存储/raft 目录未被覆盖，L2 版本链与 raft 日志原样保留
- [ ] 集群模式下全部节点 commit_index 一致且 ≥ 回滚前值

## 4. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-08-08 | 初版：CI 门禁链 + 部署脚本 + 代码级/数据级回滚流程 + 验证判据（P7-03） |
