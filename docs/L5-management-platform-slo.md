# L5 — Management Platform SLO Baseline

文档 ID: OPS-L5-0002  
版本: 1.0.0  
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

### CI 门禁

- 工作流：`.github/workflows/ci.yml`
- 作业：`check` 下的 `management server smoke`
- 判据与本地一致

---

## 4. 运行参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `ONTOLITH_MANAGEMENT_SLO_MAX_LATENCY_MS` | `250` | runtime probe 延迟阈值（ms） |
| `ONTOLITH_MANAGEMENT_SMOKE_PORT` | `19091 + RANDOM%1000` | 本地/CI smoke 使用的临时端口（避免与常驻服务冲突） |
| `ONTOLITH_MANAGEMENT_BIND` | `127.0.0.1:9091` | 管理服务监听地址 |
| `ONTOLITH_BIND` | `127.0.0.1:8080` | runtime 目标地址（probe 目标） |

---

## 5. R1 之后的扩展

- 增加时间窗口 SLO（例如 24h `availability >= 99.9%`）
- 增加 P95/P99 latency 阈值
- 增加告警策略（连续失败次数 / 延迟异常突增）
- 与 TLS/OIDC 控制面安全策略联合评估

---

## 6. 关联

- `docs/L5-ontolith-access-security.md`
- `docs/L5-systemd-service.md`
- `docs/PROGRESS.md`
- `scripts/ci-local.sh`
- `.github/workflows/ci.yml`
