# Ontolith Console

Node.js 管理组件（独立 npm 项目，不进入 Rust workspace）：前端为 **Vite 8** SPA，
后端为零依赖 Node API 服务。接入 ontolith 网关（HTTP 8080 / gRPC 50051）与管理面
（9091），提供**侧边栏菜单式**浏览器 UI 用于**观察、查询、管理**，支持**多集群
切换**与**远端访问（反代 + TLS）**。

- 观察：概览（健康/指标/监控摘要）、**实时图表（请求速率/SPARQL 错误/延迟/triples/
  commit_index/节点健康）**、集群状态、审计流、traces、架构分层
- 推理（L6 ontolith-reasoner）：推理姿态（mode/迭代/耗时上限/规则清单）、SHACL 校验
  工作台（Turtle shapes → ValidationReport）、物化运行（forward-chaining 派生统计 + 样例）
- 插件（L8 ontolith-plugin-api）：插件清单、AgentTool 工具定义（参数/必填）、契约状态
  （能力集合、manifest/agent_tool 契约版本）
- 查询：SPARQL 控制台（**HTTP / gRPC 双通道**，查询/Explain，结果表格渲染）、Turtle 写入
- 管理：管理面配置（脱敏）、审计、数据统计、集群/数据管理接口（白名单代理）
- 多集群：`clusters.json` 配置多套 ontolith 实例，顶部下拉切换，历史采样按集群隔离
- 安全：凭据仅存后端；代理白名单（防 SSRF）；可选 console 访问令牌 + 自身 TLS

## 运行

```bash
cd console
npm install                 # 安装 Vite（前端构建）
cp .env.example .env        # 单集群：填 ONTOLITH_API_KEY 等
cp clusters.example.json clusters.json   # 多集群：填入各实例凭据（不入库）

# 生产：构建前端并启动（默认 http://127.0.0.1:8890）
npm run build
npm start

# 开发：终端 1 起 API 后端，终端 2 起 Vite（HMR）
npm run dev:api             # node server.js → :8890（/api 代理、历史采样、gRPC）
npm run dev                 # Vite dev server → http://127.0.0.1:5173
```

浏览器打开 <http://127.0.0.1:8890>（生产）或 <http://127.0.0.1:5173>（开发，
Vite 将 `/api` 代理到 8890 后端）。

## 配置

### 单集群（.env）

| 变量 | 默认 | 说明 |
|------|------|------|
| `CONSOLE_BIND` | `127.0.0.1:8890` | console 监听地址 |
| `CONSOLE_STATIC_DIR` | `dist/`（无则 `src/`） | server.js 托管的静态目录 |
| `ONTOLITH_GATEWAY_URL` | `http://127.0.0.1:8080` | 网关基址 |
| `ONTOLITH_MANAGEMENT_URL` | `http://127.0.0.1:9091` | 管理面基址 |
| `ONTOLITH_GRPC_URL` | `http://127.0.0.1:50051` | gRPC 端点（HTTP/2 h2c） |
| `ONTOLITH_API_KEY` | — | **必填**（无 clusters.json 时） |
| `ONTOLITH_TENANT` / `ONTOLITH_USER` | `prod` / `console` | 注入的鉴权头 |
| `CONSOLE_REFRESH_MS` | `5000` | 自动刷新/采样间隔 |
| `CONSOLE_HISTORY_POINTS` | `120` | 历史采样窗口 |
| `CONSOLE_AUTH_TOKEN` | 空 | 设置后所有 `/api/*` 路由要求 `Authorization: Bearer <token>`（静态资源公开，前端显示登录框） |
| `CONSOLE_TLS_CERT` / `CONSOLE_TLS_KEY` | 空 | 设置后以 HTTPS 监听 |

### 多集群（clusters.json）

存在 `clusters.json` 时优先于 .env（.env 仅用于缺省单集群）。每项：

```json
{ "id": "prod", "name": "Production", "gateway": "http://127.0.0.1:8080",
  "management": "http://127.0.0.1:9091", "grpc": "http://127.0.0.1:50051",
  "apiKey": "...", "tenant": "prod", "user": "console" }
```

`clusters.json` 含密钥，已 gitignore；模板见 `clusters.example.json`。

## 架构

```
浏览器 ──> /（Vite 构建的静态 SPA，侧边栏菜单 + 集群下拉 + 登录令牌）
         ├─> /api/clusters ── 集群清单（不含密钥）
         ├─> /api/health?cluster=<id> ── 指定/全部集群可达性
         ├─> /api/history?cluster=<id> ── 时序采样（环形缓冲）
         ├─> /api/gw/<cluster>/<path> ── 网关白名单代理（health/metrics/sparql/explain/
         │                                 cluster*/semantic*/inference|validate/shacl|
         │                                 materialize/data/*）
         ├─> /api/mg/<cluster>/<path> ── 管理面白名单代理（admin/health|config|layers|
         │                                 plugins|monitoring|traces|data/stats|data/audit|
         │                                 data/replicate|data/rebalance）
         └─> /api/grpc/<cluster>/{health|query} ── 零依赖 gRPC 通道（node:http2 +
                                                 手写 protobuf，SparqlService）
```

- 凭据只存在于后端（`.env` / `clusters.json`，chmod 600），前端不接触 API key。
- 代理路径**白名单精确匹配**，非白名单 404；gRPC 仅开放 `Health`/`Query`。
- 历史采样由后端按集群独立维护，前端 Canvas 零依赖绘制。

## 远端访问（反代 + TLS）

console 默认只监听 `127.0.0.1`。暴露到远端时建议：

1. 为 console 开启**访问令牌**（`CONSOLE_AUTH_TOKEN`）——未带
   `Authorization: Bearer <token>` 的请求一律 401（前端会显示登录框）。
2. 用反代终结 TLS 并转发（console 自身也可启用 TLS，见 .env）。

nginx 示例（`/etc/nginx/sites-available/ontolith-console`）：

```nginx
server {
  listen 443 ssl;
  server_name console.example.com;
  ssl_certificate     /etc/letsencrypt/live/console.example.com/fullchain.pem;
  ssl_certificate_key /etc/letsencrypt/live/console.example.com/privkey.pem;

  location / {
    proxy_pass http://127.0.0.1:8890;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
  }
}
```

Caddy 示例（`Caddyfile`）：

```
console.example.com {
    reverse_proxy 127.0.0.1:8890
}
```

建议同时：`limit_req` 限流、IP 白名单、`CONSOLE_AUTH_TOKEN` 强口令。
注意：console 的 Bearer 令牌是访问控制层，ontolith 自身的 API key 仍由后端持有，
不会被暴露给浏览器。

## 开发

```bash
npm run check          # 语法校验（server.js / grpc.js / vite.config.js / src/app.js）
npm run dev:api        # 后端 API（:8890）：/api 代理、历史采样、gRPC 通道
npm run dev            # Vite dev server（:5173，HMR），/api 自动代理到后端
npm run build          # 产出 dist/，npm start 时由 server.js 托管
```

## 与本仓库的关系

- 生产部署目录：`/home/ontolith/prod/`（RocksDB + AUTH enforced + 审计落盘，
  见 `docs/RELEASE-2026-08-09.md`）；staging 演练实例：`18080/19091/15051`。
- 真机部署 console：以 systemd/supervisor 守护 `node server.js`，配置同上。
