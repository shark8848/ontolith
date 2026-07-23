# L5 — Access Layer & Security Baseline

文档 ID: IMPL-L5-0001  
版本: 2.2.0  
状态: Implemented (HTTP + dual backend + file audit + SPARQL Results JSON + management server)  
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

租户图隔离（可选）：

```http
POST /data/nt?tenant_graph=1
X-Ontolith-Tenant: acme
```

语句写入命名图 `urn:tenant:acme`。

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
| `ONTOLITH_API_KEY` | — | Enforced 时校验 |
| `ONTOLITH_MANAGEMENT_READ_KEY` | — | 管理面只读 key（可选） |
| `ONTOLITH_MANAGEMENT_WRITE_KEY` | — | 管理面写操作 key（可选） |

```bash
# 内存
cargo run -p ontolith-server

# RocksDB 耐久
ONTOLITH_STORAGE=rocksdb ONTOLITH_DATA_DIR=./data/ontolith cargo run -p ontolith-server

# 管理服务（统一管理面）
cargo run -p ontolith-server --bin ontolith-management-server
```

实现：`AppState` 持有 `Arc<dyn StorageEngine>` + `Arc<dyn DictionaryCodec>` + 通用 `EngineTripleRepository`。

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
- 写入可选 tenant 命名图  

---

## 6. 测试

| Crate | 数量 | 覆盖 |
|-------|------|------|
| ontolith-security | 5 | 鉴权/权限/审计 |
| ontolith-server | **8** | turtle 写入、SPARQL JSON、tenant graph、强制鉴权、**RocksDB reopen** |

---

## 7. 已知限制

1. 无 TLS / HTTP/2 / 完整框架中间件链  
2. 鉴权非 OIDC/JWT  
3. 审计仍进程内（不落盘）  
4. 租户隔离为可选命名图，非强制全库分片  
5. SPARQL Results JSON 为兼容子集（非完整 XML/CSV）  

---

## 8. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | HTTP 基线路由 + Header 鉴权 |
| 2026-07-17 | 2.0.0 | RocksDB 切换、L3 解析写入、SPARQL Results JSON、/ready、增强 metrics、tenant graph |
| 2026-07-23 | 2.2.0 | 新增独立 `ontolith-management-server` 管理面（二进制 + 统一配置/监控/数据管理 API） |
| 2026-07-23 | 2.2.1 | 管理面 ACL 分离：支持 read/write key 双轨控制（`X-Ontolith-Management-Key`） |

## 8. 审计落盘与权限（v2.1）

| 能力 | 说明 |
|------|------|
| 内存审计 | `InMemoryAuditLog`（请求路径默认） |
| 文件审计 | `FileAuditLog` JSONL；`ONTOLITH_AUDIT_PATH` 或 rocksdb 时 `$DATA_DIR/audit.jsonl` |
| 权限 | 默认角色含 `cluster:admin`；集群写路径要求该权限 |

环境变量：

```bash
ONTOLITH_AUDIT_PATH=/path/to/audit.jsonl   # 可选
ONTOLITH_AUTH_MODE=enforced
ONTOLITH_API_KEY=...
```

## 9. 变更
