# L5 — Access Layer & Security Baseline

文档 ID: IMPL-L5-0001  
版本: 2.5.0  
状态: Implemented (HTTP + dual backend + file audit + SPARQL Results JSON + management server + enforced tenant isolation P5-03)  
日期: 2026-07-23  
对应 crate:

- `crates/ontolith-server`
- `crates/ontolith-security`
- （消费）`ontolith-query` / `ontolith-storage` / `ontolith-parser` / `ontolith-observability`

---

## 1. 层定位

```text
Clients
   │  HTTP/1.1
   ▼
ontolith-server (L5 gateway)
   ├── security: auth + audit + tenant context
   ├── parser ingest (L3)
   ├── query pipeline (L3)
   └── storage: memory | rocksdb (L2)

ontolith-management-server (L5 management plane)
   ├── config view: binds/auth/backend/audit path
   ├── monitoring view: request, latency, cluster status
   ├── data management: stats/audit/replicate/rebalance
   └── shared authn/authz + shared AppState
```

---

## 2. HTTP API

| Method | Path | 权限 | 说明 |
|--------|------|------|------|
| GET | `/health` `/healthz` | health:read | 存活、backend、triples/quads |
| GET | `/ready` `/readyz` | health:read | 就绪探针 |
| GET | `/metrics` | metrics:read | Prometheus（含延迟/状态码/存储） |
| GET | `/audit` | metrics:read | 审计 JSON（`?limit=`） |
| GET/POST | `/sparql` | sparql:query | SPARQL Results JSON |
| GET/POST | `/explain` | sparql:explain | 计划 Explain JSON |
| POST | `/data` `/data/nt` `/data/turtle` `/data/trig` `/data/nq` | data:write | 完整 L3 解析写入 |
| GET | `/cluster` `/cluster/status` `/cluster/membership` `/cluster/shards` `/cluster/route` `/cluster/failover` | health:read | L4 控制面只读 |
| POST | `/cluster/heartbeat` `/tick` `/replicate` `/rebalance` `/partition` `/heal` | cluster:admin | L4 控制面变更 |
| OPTIONS | * | — | CORS |

### 管理面 API（`ontolith-management-server`）

| Method | Path | 权限 | 说明 |
|--------|------|------|------|
| GET | `/admin/health` | health:read | 管理服务健康与启动时间 |
| GET | `/admin/config` | cluster:admin | 统一配置视图（bind/backend/auth/audit） |
| GET | `/admin/layers` | cluster:admin | L0–L8 层级映射与职责 |
| GET | `/admin/monitoring` | metrics:read | 请求/延迟/状态码/集群摘要 |
| GET | `/admin/data/stats` | health:read | triples/quads/pending_txns/audit 总量 |
| GET | `/admin/data/audit` | metrics:read | 审计事件检索（`?limit=`） |
| POST | `/admin/data/replicate` | cluster:admin | 触发 follower 复制对齐 |
| POST | `/admin/data/rebalance` | cluster:admin | 触发 slot 重平衡 |

管理面监控会探测运行时地址 `ONTOLITH_BIND` 的连通性（TCP connect），并在
`/admin/health` 与 `/admin/monitoring` 返回 `runtime_probe` 信息（reachable/latency/error）。

管理面 ACL（可选）：

- `ONTOLITH_MANAGEMENT_READ_KEY`：允许读取管理视图
- `ONTOLITH_MANAGEMENT_WRITE_KEY`：允许管理变更（`POST /admin/data/*`）
- 请求头：`X-Ontolith-Management-Key`

### SPARQL

| 来源 | 参数 |
|------|------|
| Query | `query`, `timeout_ms`, `explain=1`, `format=json` |
| Body | `application/sparql-query` / form `query=` / raw |
| Header | `X-Ontolith-Timeout-Ms`, `X-Ontolith-Explain`, `X-Ontolith-Consistency` |

响应（SELECT）对齐 W3C SPARQL Results JSON 形态：

```json
{
  "head": { "vars": ["s","p","o"] },
  "results": { "bindings": [ { "s": {"type":"uri","value":"..."} } ] },
  "meta": { "row_count": 1, "elapsed_ms": 0, "tenant": "...", "consistency": "strong" }
}
```

ASK → `{ "boolean": true/false, "meta": {...} }`  
CONSTRUCT → `{ "results": { "triples": [...], "count": N } }`

### 写入 / 解析

| 方式 | 格式 |
|------|------|
| path | `/data/nt` `/data/turtle` `/data/trig` `/data/nq` |
| `?format=` | `nt` `turtle` `trig` `nq` |
| Content-Type | `text/turtle`, `application/trig`, `application/n-triples`, `application/n-quads` |

租户图隔离：

```http
# 可选模式（默认）：请求级选择
POST /data/nt?tenant_graph=1
X-Ontolith-Tenant: acme

# 强制模式（P5-03）：`ONTOLITH_TENANT_MODE=enforced`
POST /data/nt
X-Ontolith-Tenant: acme
```

语句写入命名图 `urn:tenant:acme`。强制模式下隔离为**分库/行级**：

- 写路径无条件盖章：默认图语句写入 `urn:tenant:<t>`；`?graph=`/TriG/NQuads 命名图引用必须在 `urn:tenant:<t>` 命名空间内，越权返回 403
- 读路径（SPARQL）注入 `tenant_scope`：默认图重指向租户图，`FROM`/`GRAPH`/`USING`/图管理目标越权返回 403，跨租户数据结构性不可达
- `X-Ontolith-Tenant`/`X-Ontolith-User`/`X-API-Key` 仍由鉴权层强制（`ONTOLITH_AUTH_MODE=enforced`）

### 鉴权（`ONTOLITH_AUTH_MODE=enforced`）

| Header | 含义 |
|--------|------|
| `X-API-Key` | 匹配 `ONTOLITH_API_KEY` |
| `X-Ontolith-Tenant` | 租户（强制） |
| `X-Ontolith-User` | 用户（强制） |

---

## 3. 存储后端切换

| 环境变量 | 默认 | 说明 |
|----------|------|------|
| `ONTOLITH_STORAGE` | `memory` | `memory` \| `rocksdb` / `durable` |
| `ONTOLITH_DATA_DIR` | `./data/ontolith` | RocksDB 路径 |
| `ONTOLITH_BIND` | `127.0.0.1:8080` | 监听地址 |
| `ONTOLITH_MANAGEMENT_BIND` | `127.0.0.1:9091` | 管理服务监听地址 |
| `ONTOLITH_AUTH_MODE` | `disabled` | `disabled` \| `enforced` |
| `ONTOLITH_TENANT_MODE` | `disabled` | `disabled` \| `enforced`（P5-03 强制租户隔离：写盖章 + 读作用域 + 越权 403） |
| `ONTOLITH_API_KEY` | — | Enforced 时校验 |
| `ONTOLITH_MANAGEMENT_READ_KEY` | — | 管理面只读 key（可选） |
| `ONTOLITH_MANAGEMENT_WRITE_KEY` | — | 管理面写操作 key（可选） |
| `ONTOLITH_MANAGEMENT_PROBE_TIMEOUT_MS` | `300` | 管理面 runtime 探测超时（毫秒） |
| `ONTOLITH_TLS_CERT` | — | 管理面 TLS 证书链 PEM（与 KEY 同设启用 rustls 进程内终止） |
| `ONTOLITH_TLS_KEY` | — | 管理面 TLS 私钥 PEM（非 loopback bind 强制要求，R2 门禁） |

```bash
# 内存
cargo run -p ontolith-server

# RocksDB 耐久
ONTOLITH_STORAGE=rocksdb ONTOLITH_DATA_DIR=./data/ontolith cargo run -p ontolith-server

# 管理服务（统一管理面）
cargo run -p ontolith-server --bin ontolith-management-server
```

实现：`AppState` 持有 `Arc<dyn StorageEngine>` + `Arc<dyn DictionaryCodec>` + 通用 `EngineTripleRepository`。

### 传输加密（TLS 终止，ADR-0003）

管理服务支持 rustls 进程内 TLS 终止：

```bash
# 自签证书（或使用正式 CA 证书）
./scripts/gen-self-signed-cert.sh --cn mgmt.example.com /etc/ontolith/tls

# 启动 HTTPS 管理面
ONTOLITH_TLS_CERT=/etc/ontolith/tls/ontolith.crt.pem \
ONTOLITH_TLS_KEY=/etc/ontolith/tls/ontolith.key.pem \
cargo run -p ontolith-server --bin ontolith-management-server
```

- 仅设置其一会导致启动失败；证书/私钥 PEM 解析失败同样拒绝启动。
- **R2 强制门禁**：`ONTOLITH_MANAGEMENT_BIND` 解析为非 loopback 地址且未配置 TLS 时，进程拒绝启动（`enforce_tls_gate`）。
- 绑定姿态证据：`GET /admin/config` 返回 `"tls":"on"|"off"`。
- 反向代理/ingress 前置 TLS 终止仍受支持（服务保持 loopback bind 即不触发门禁）。

### 后台 / systemd

详见 [L5-systemd-service.md](./L5-systemd-service.md)。

```bash
# 用户服务（无需 root）
cargo build -p ontolith-server --release
./scripts/install-ontolith-user-service.sh
systemctl --user status ontolith-server

# 系统服务（需 sudo）
./scripts/install-ontolith-system-service.sh
```

---

## 4. 可观测性

`/metrics` 暴露：

- `ontolith_http_requests_total`
- `ontolith_sparql_requests_total` / `ontolith_sparql_errors_total`
- `ontolith_ingest_requests_total`
- `ontolith_http_request_latency_ms_{sum,count,avg}`
- `ontolith_http_responses_total{status=...}`
- `ontolith_storage_{triples,quads,pending_txns}`
- `ontolith_audit_events`

每个请求 stderr access log：`method path status latency_ms bytes`。

---

## 5. 安全模型

- Deny-by-default 权限  
- Disabled → system admin  
- Enforced → API key + tenant/user  
- 审计 allow/deny；`/audit` 租户过滤  
- 写入可选 tenant 命名图；`ONTOLITH_TENANT_MODE=enforced` 时为强制分库/行级隔离（写盖章 + 读作用域 + 越权 403）  

---

## 6. 测试

| Crate | 数量 | 覆盖 |
|-------|------|------|
| ontolith-security | 12 | 鉴权/权限/审计（含哈希链完整性验证）+ `TenantMode`/`TenantNamespace` 命名空间校验 |
| ontolith-server | **24** | turtle 写入、SPARQL JSON、tenant graph、强制鉴权、**RocksDB reopen**、**TLS 终止（rustls 往返）**、**R2 非 loopback TLS 门禁**、**强制租户隔离（acme/other 互不可见、越权引用 403、默认图写盖章）** |

---

## 7. 已知限制

1. TLS 已落地（rustls 进程内终止 + R2 非 loopback 门禁）；仍无 HTTP/2 / 完整框架中间件链  
2. 鉴权非 OIDC/JWT  
3. 审计哈希链为完整性级（FNV-1a 64，非加密级；加密升级保持同 schema）  
4. 租户隔离已升级为强制分库/行级（`ONTOLITH_TENANT_MODE=enforced`：命名图命名空间隔离 + 执行器租户视图）；分库物理隔离（每租户独立 RocksDB 实例）仍为后续增强  
5. SPARQL Results JSON 为兼容子集（非完整 XML/CSV）  

---

## 8. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | HTTP 基线路由 + Header 鉴权 |
| 2026-07-17 | 2.0.0 | RocksDB 切换、L3 解析写入、SPARQL Results JSON、/ready、增强 metrics、tenant graph |
| 2026-07-23 | 2.2.0 | 新增独立 `ontolith-management-server` 管理面（二进制 + 统一配置/监控/数据管理 API） |
| 2026-07-23 | 2.2.1 | 管理面 ACL 分离：支持 read/write key 双轨控制（`X-Ontolith-Management-Key`） |
| 2026-07-23 | 2.2.2 | 管理面 runtime probe：健康/监控响应增加运行时连通性与探测延迟信息 |
| 2026-08-06 | 2.3.0 | 审计哈希链：`FileAuditLog` 每条追加 `prev`/`hash` 字段（FNV-1a 64，genesis=0），reopen 恢复链尾，新增 `verify_chain()` 全链校验与篡改检测，+2 测 |
| 2026-08-06 | 2.4.0 | 管理面 TLS 终止（rustls）+ R2 非 loopback TLS 强制门禁（ADR-0003 转 Accepted）：`HttpServer::with_tls`/`TlsServerConfig`、`ONTOLITH_TLS_CERT`/`ONTOLITH_TLS_KEY`、`/admin/config` 暴露 `tls` 姿态、`gen-self-signed-cert.sh`，+4 测 |

## 8. 审计落盘与权限（v2.1）

| 能力 | 说明 |
|------|------|
| 内存审计 | `InMemoryAuditLog`（请求路径默认） |
| 文件审计 | `FileAuditLog` JSONL；`ONTOLITH_AUDIT_PATH` 或 rocksdb 时 `$DATA_DIR/audit.jsonl` |
| 哈希链 | 每条 JSONL 含 `prev`/`hash`（FNV-1a 64）；`verify_chain()` 全链校验 |
| 权限 | 默认角色含 `cluster:admin`；集群写路径要求该权限 |

环境变量：

```bash
ONTOLITH_AUDIT_PATH=/path/to/audit.jsonl   # 可选
ONTOLITH_AUTH_MODE=enforced
ONTOLITH_API_KEY=...
```

## 9. 变更

| 2026-08-08 | 2.5.0 | **P5-03 强制租户隔离**：`TenantMode`/`TenantNamespace`（`ONTOLITH_TENANT_MODE`，`urn:tenant:<t>` 命名空间 + `require_owned` 403）；`QueryRequest.tenant_scope` 下沉执行器（`TenantScopedRead/Write`：默认图重指向 + 命名图过滤 + 更新盖章），`FROM`/`GRAPH`/`USING`/图管理目标越权 403；server 写路径强制盖章、读路径注入作用域、`/health`+`/admin/config` 暴露 `tenant_mode`；security 9→12、query 83→86、server 22→24 测 |