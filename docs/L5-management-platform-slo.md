# L5 — Management Platform SLO Baseline

文档 ID: OPS-L5-0002  
版本: 1.1.0  
状态: Active (R1 baseline)  
日期: 2026-07-23

---

## 1. 目标

定义管理平台（`ontolith-management-server`）在 R1 阶段的最小可观测服务目标（SLO），并把阈值接入本地与 CI smoke 门禁。

---

## 2. SLI / SLO 定义（R1）

### SLI-1: 管理面健康可达性

- 指标来源：`GET /admin/health`
- 判定：HTTP 200 + 响应包含 `status=ok`

SLO（R1）：

- 本地/CI smoke 单次检查通过率 = 100%
- 若 20 秒内无法通过健康检查，门禁失败

### SLI-2: runtime_probe 连通性

- 指标来源：`GET /admin/monitoring` -> `runtime_probe.reachable`
- 判定：`runtime_probe.reachable == true`

SLO（R1）：

- 本地/CI smoke 单次检查通过率 = 100%

### SLI-3: runtime_probe 延迟

- 指标来源：`GET /admin/monitoring` -> `runtime_probe.latency_ms`
- 判定：`latency_ms <= ONTOLITH_MANAGEMENT_SLO_MAX_LATENCY_MS`

SLO（R1）：

- 默认阈值：`250ms`
- 可通过环境变量 `ONTOLITH_MANAGEMENT_SLO_MAX_LATENCY_MS` 覆盖

---

## 3. 门禁接入点

### 本地门禁

- 脚本：`scripts/ci-local.sh`
- 流程：
  1. 启动 `ontolith-management-server`
  2. 轮询 `/admin/health`
  3. 校验 `/admin/monitoring` 中 `runtime_probe.reachable=true`
  4. 校验 `runtime_probe.latency_ms` 不超过阈值
  5. 执行短窗口 SLO 检查脚本（默认 5 样本、0 秒间隔）

### CI 门禁

- 工作流：`.github/workflows/ci.yml`
- 作业：`check` 下的 `management server smoke`
- 判据与本地一致

### 窗口化 SLO 检查脚本

- 脚本：`scripts/check-management-slo-window.sh`
- 目标：在短时间窗口内验证 `runtime_probe` 成功率与 P95 延迟是否满足阈值
- 默认阈值：
  - `success_percent >= 99`
  - `p95_latency_ms <= 250`

---

## 4. 运行参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `ONTOLITH_MANAGEMENT_SLO_MAX_LATENCY_MS` | `250` | runtime probe 延迟阈值（ms） |
| `ONTOLITH_MANAGEMENT_SMOKE_PORT` | `19091 + RANDOM%1000` | 本地/CI smoke 使用的临时端口（避免与常驻服务冲突） |
| `ONTOLITH_MANAGEMENT_MONITORING_URL` | `http://127.0.0.1:9091/admin/monitoring` | 窗口检查读取地址 |
| `ONTOLITH_MANAGEMENT_SLO_WINDOW_SAMPLES` | `12` | 窗口采样次数 |
| `ONTOLITH_MANAGEMENT_SLO_WINDOW_INTERVAL_SEC` | `5` | 采样间隔（秒） |
| `ONTOLITH_MANAGEMENT_SLO_MIN_SUCCESS_PERCENT` | `99` | 成功率阈值（百分比） |
| `ONTOLITH_MANAGEMENT_SLO_P95_MAX_LATENCY_MS` | `250` | P95 延迟阈值（ms） |
| `ONTOLITH_MANAGEMENT_BIND` | `127.0.0.1:9091` | 管理服务监听地址 |
| `ONTOLITH_BIND` | `127.0.0.1:8080` | runtime 目标地址（probe 目标） |

---

## 5. 天/周窗口 SLO 自动化（1.1.0）

### 采集与历史

- 采集脚本：`scripts/collect-slo-sample.sh`
  - 读取 `GET /admin/monitoring` 的 `runtime_probe.reachable` / `latency_ms`
  - 追加 JSONL 到 `ONTOLITH_SLO_STATE_DIR`（默认 `~/.local/state/ontolith/slo/samples.jsonl`）
  - 服务不可达时记录 `reachable=false` 样本（不中断采集）
- systemd user timer：`ontolith-slo-collect.timer`（每 5 分钟）→ `ontolith-slo-collect.service`

### 窗口评估与告警策略

- 评估脚本：`scripts/check-slo-window-history.sh --window-hours N`
  - 窗口内成功率：`success_percent = ok / (ok + fail)`
  - 窗口内 P95 延迟（与现有 `check-management-slo-window.sh` 同秩公式）
  - 连续失败：窗口内末尾连续 `reachable=false` 样本数
  - 延迟突增：本窗口 P95 > 前窗口 P95 × `ONTOLITH_SLO_LATENCY_SPIKE_FACTOR`
- 触发条件（任一即 breach，退出码 1）：
  1. `success_percent < ONTOLITH_SLO_MIN_SUCCESS_PERCENT`（默认 99）
  2. 连续失败 ≥ `ONTOLITH_SLO_MAX_CONSECUTIVE_FAILURES`（默认 3）
  3. 窗口 P95 > `ONTOLITH_SLO_P95_MAX_LATENCY_MS`（默认 250）
  4. P95 延迟突增超系数（默认 2.0×）
- 每次评估追加报告到 `reports.jsonl`；breach 时追加 `alerts.jsonl`（供外部 webhook/邮件钩子消费）
- systemd timers：
  - `ontolith-slo-daily.timer`（每日 00:05，`--window-hours 24`）
  - `ontolith-slo-weekly.timer`（周一 00:10，`--window-hours 168`）
- 安装：`bash scripts/install-ontolith-slo-timers.sh`（user systemd，免 root）

### 自测

- `bash scripts/check-slo-window-history.sh --self-test`
  - 4 个确定性用例：健康窗口通过、连续失败 breach、P95 超阈值 breach、延迟尖峰 breach
  - 已接入 `scripts/ci-local.sh`（无需起服务）

### 运行参数（天/周窗口）

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `ONTOLITH_SLO_STATE_DIR` | `~/.local/state/ontolith/slo` | samples/reports/alerts 历史目录 |
| `ONTOLITH_SLO_WINDOW_HOURS` | `24` | 窗口长度（小时），周窗口用 `168` |
| `ONTOLITH_SLO_MIN_SUCCESS_PERCENT` | `99` | 窗口成功率阈值 |
| `ONTOLITH_SLO_P95_MAX_LATENCY_MS` | `250` | 窗口 P95 延迟阈值 |
| `ONTOLITH_SLO_MAX_CONSECUTIVE_FAILURES` | `3` | 连续失败告警阈值 |
| `ONTOLITH_SLO_LATENCY_SPIKE_FACTOR` | `2.0` | 与前窗口 P95 的尖峰倍数 |

---

## 5. R1 实测基线（2026-08-08）

核心 SLO 基线达标（R1 检查项）实测证据：本机启动 `ontolith-management-server`
（debug 构建，`ONTOLITH_MANAGEMENT_BIND`/`ONTOLITH_BIND` 指向同一随机端口），
用现有采集/评估脚本走真实观测窗口。

### 短窗口 SLO（`check-management-slo-window.sh`）

- 配置：20 样本 × 1s 间隔，阈值 success% >= 99、p95 <= 250ms
- 结果：**通过**（20/20 可达，success 100%，p95 = 0ms）

### 天/周窗口历史 SLO（`collect-slo-sample.sh` + `check-slo-window-history.sh`）

- 配置：20 样本 × 1s 间隔写入 `samples.jsonl`，`--window-hours 1` 评估
  （阈值 success% >= 99、p95 <= 250ms、连续失败 < 3、尖峰 <= 2.0×）
- 结果：**通过**，报告 `{"samples":20,"success_percent":100,"p95_latency_ms":0,
  "consecutive_failures":0,"passed":1}`

### 实测延迟分布（20 样本）

| 指标 | 值 |
|------|-----|
| min / p50 / p95 / max（ms） | 0 / 0 / 0 / 3 |
| 目标阈值 p95（ms） | <= 250 |

结论：R1 核心 SLO 基线已达标（远优于阈值）；门禁由 CI `management server smoke`
与 `bench` 作业持续守护。长期天/周窗口由 systemd timers 采集并评估
（`scripts/install-ontolith-slo-timers.sh`）。

---

## 5. R1 之后的扩展

- 增加时间窗口 SLO（例如 24h `availability >= 99.9%`）
- 增加 P95/P99 latency 阈值
- 增加告警策略（连续失败次数 / 延迟异常突增）
- 与 TLS/OIDC 控制面安全策略联合评估
- 将窗口检查脚本接入 systemd timer 或 Prometheus Alert 规则

---

## 6. 关联

- `docs/L5-ontolith-access-security.md`
- `docs/L5-systemd-service.md`
- `docs/PROGRESS.md`
- `scripts/ci-local.sh`
- `scripts/check-management-slo-window.sh`
- `.github/workflows/ci.yml`
