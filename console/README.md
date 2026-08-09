# Ontolith Console

零依赖 Node.js 管理组件（独立 npm 项目，不进入 Rust workspace）。接入 ontolith
网关（HTTP 8080）与管理面（9091），提供浏览器 UI 用于**观察、查询与管理**。

- 观察：网关/管理面健康、Prometheus 指标、集群状态、数据统计、审计流、traces、架构分层
- 查询：SPARQL 控制台（查询 / Explain，结果表格渲染）、Turtle 写入
- 管理：管理面配置（脱敏）、`/admin/data/audit`、集群/数据管理接口（经白名单代理）

## 运行

```bash
cd console
cp .env.example .env        # 填入 ONTOLITH_API_KEY 等
npm start                   # 默认 http://127.0.0.1:8890
```

浏览器打开 <http://127.0.0.1:8890>。

## 配置（.env）

| 变量 | 默认 | 说明 |
|------|------|------|
| `CONSOLE_BIND` | `127.0.0.1:8890` | console 监听地址（本机访问） |
| `ONTOLITH_GATEWAY_URL` | `http://127.0.0.1:8080` | 网关基址 |
| `ONTOLITH_MANAGEMENT_URL` | `http://127.0.0.1:9091` | 管理面基址 |
| `ONTOLITH_API_KEY` | — | **必填**，ontolith `AUTH_MODE=enforced` 的 API key |
| `ONTOLITH_TENANT` | `prod` | 注入的租户头 |
| `ONTOLITH_USER` | `console` | 注入的用户头 |
| `CONSOLE_REFRESH_MS` | `5000` | 仪表盘自动刷新间隔（0 关闭） |

## 架构

```
浏览器 ──> /（静态 SPA）
         └─> /api/gw/<path> ──> 网关   （白名单：health/ready/metrics/audit/sparql/explain/
                                    cluster*/semantic*/data/*）
         └─> /api/mg/<path> ──> 管理面  （白名单：admin/health|config|layers|monitoring|
                                    traces|data/stats|data/audit|data/replicate|data/rebalance）
         └─> /api/health ──> 双端可达性汇总
```

- **凭据只存在于后端**（`.env`，chmod 600），前端不接触 API key。
- 代理路径为**白名单精确匹配**，非白名单路径返回 404（防 SSRF/横向路径）。
- 仅监听 `127.0.0.1`，本机访问；如需远端访问请置于反向代理 + TLS 之后。
- 写操作（Turtle 写入、replicate/rebalance）已包含在白名单中，前端操作前有确认。

## 开发

```bash
npm run check      # node --check 语法校验（零依赖，无构建步骤）
```

## 与本仓库的关系

- 生产部署目录：`/home/ontolith/prod/`（RocksDB + AUTH enforced + 审计落盘，见
  `docs/RELEASE-2026-08-09.md`）。
- 真机部署 console 时：`node server.js` 以 systemd/supervisor 守护，配置同上。
