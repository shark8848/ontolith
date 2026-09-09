# L5 — Access Layer & Security Baseline

文档 ID: IMPL-L5-0001  
版本: 2.15.0  
状态: Implemented (HTTP + gRPC + dual backend + file audit（SHA-256 哈希链）+ SPARQL Results JSON + management server + enforced tenant isolation P5-03 + OIDC 完整链路 R2+（JWKS 校验 + TTL 缓存刷新）+ full-chain tracing P5-05 + 租户管理（注册表 CRUD + 管理面代理）+ Ontology 载荷联动推理输入)  
日期: 2026-09-09  
对应 crate:

- `crates/ontolith-server`
- `crates/ontolith-security`
- （消费）`ontolith-query` / `ontolith-storage` / `ontolith-parser` / `ontolith-observability`

---

## 1. 层定位

```text
Clients
   │  HTTP/1.1 │ gRPC (HTTP/2)
   ▼
ontolith-server (L5 gateway)
   ├── grpc: tonic SparqlService{Query,Health} (P5-01)
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
| GET/POST | `/sparql` | sparql:query | SPARQL Results JSON；更新支持远程 `LOAD <http(s)://…>`（server 层 `http_get` 抓取 + 格式探测 + 图感知 `INSERT DATA` 原位替换；SILENT/INTO GRAPH/命名图保留；enforced 租户 owned-graph 校验；`https://` 自 2026-09-09 起经树内 rustls 客户端）；查询另支持 SPARQL 1.1 `SERVICE <http(s)://…>` 联邦（请求内注入 `HttpServiceClient`，依赖式 dispatch + SILENT + VALUES 下推；`http://`/`https://` 均支持） |
| GET/POST | `/explain` | sparql:explain | 计划 Explain JSON |
| POST | `/data` `/data/nt` `/data/turtle` `/data/trig` `/data/nq` `/data/rdfxml`（别名 `rdf-xml`/`rdf`） | data:write | 完整 L3 解析写入（含 RDF/XML） |
| GET | `/data` | data:read | Fuseki 风格数据集导出（`?format=`/`Accept`：`ttl`/`trig`/`nq`/`nt`/`rdf+xml`；TenantMode=enforced 下仅导出调用方 owned graphs；默认图视图 = owned 图并集） |
| GET | `/cluster` `/cluster/status` `/cluster/membership` `/cluster/shards` `/cluster/route` `/cluster/failover` | health:read | L4 控制面只读 |
| POST | `/cluster/heartbeat` `/tick` `/replicate` `/rebalance` `/partition` `/heal` | cluster:admin | L4 控制面变更 |
| GET/POST | `/admin/tenants` | cluster:admin | 租户列表 / 创建（仅 `system` 租户可管理，见 §2.1） |
| PUT/DELETE | `/admin/tenants/<id>` | cluster:admin | 更新（名称/描述/状态）/ 删除租户 |
| POST | `/admin/tenants/<id>/keys` | cluster:admin | 生成 API key（原始值仅返回一次，落库存摘要） |
| DELETE | `/admin/tenants/<id>/keys/<key_id>` | cluster:admin | 吊销租户 API key |
| OPTIONS | * | — | CORS |

### 2.1 租户管理（网关 `/admin/tenants*`）

租户注册表由网关承载（RocksDB 后端时落独立 `tenant` CF，`ONTOLITH_STORAGE=memory`
时走内存实现），管理面与 console 均通过代理访问网关——**RocksDB 只能被单一进程
打开**，因此管理面不直连注册表存储。

- 鉴权：仅 `x-ontolith-tenant: system` + `cluster:admin` 可管理；非 `system`
  租户请求返回 403（防租户自管理）。
- 存储布局（RocksDB）：`t:<id>` → 租户 JSON；`k:<key_digest>` → 租户 id（digest
  为 FNV-1a 64 十六进制，原始 key 不落盘）。
- API key 格式：`ontk_<16 hex>`；生成时校验唯一性（摘要碰撞即拒绝重发）。
- 租户 JSON 字段：`id`、`name`、`description`、`status`（`active`/`disabled`）、
  `api_keys`（`key_id`/`label`/`digest`/`created_at_ms`）、`created_at_ms`、
  `updated_at_ms`；id 校验 `[a-z0-9_-]` 1..64，`system` 保留不可创建。
- CRUD 即时生效：鉴权器与注册表共享同一 `Arc`，无需重启。

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
| GET | `/admin/tenants` `/admin/tenants/<id>` | authorize_admin_view | 租户列表/详情（ACL 代理到网关） |
| POST/PUT/DELETE | `/admin/tenants*` | authorize_admin_mutation | 租户变更（ACL 代理到网关） |

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

### gRPC（P5-01，tonic HTTP/2）

`proto/ontolith/v1/sparql.proto` → `SparqlService{Query, Health}`（`grpc-backend` feature，默认开）。鉴权/租户/追踪与 HTTP 网关同构：

| RPC | 输入 | 输出 |
|-----|------|------|
| `Query` | `QueryRequest{query, format, explain, timeout_ms, consistency}` | `QueryResponse{ok, http_status, body, error}`（body 为 SPARQL Results JSON 或 explain JSON） |
| `Health` | `HealthRequest` | `HealthResponse{status, backend, tenant_mode, auth_mode, jwt, tracing}` |

- 鉴权 metadata：`x-ontolith-tenant` / `x-ontolith-user` / `x-api-key` / `authorization`（enforced 缺失 → `Unauthenticated`，跨租户图 → `PermissionDenied`）
- 追踪：请求 metadata `traceparent` 延续；响应 metadata 回带 `traceparent`；根 span `grpc.request` + 子 span `http.auth`/`sparql.execute`/`grpc.query` 进入共享 trace store
- 监听：`ONTOLITH_GRPC_BIND`（默认 `127.0.0.1:50051`）；`ontolith-server` bin 同时服务 HTTP（`ONTOLITH_BIND`）与 gRPC 双网关，共享 `AppState`

### 写入 / 解析

| 方式 | 格式 |
|------|------|
| path | `/data/nt` `/data/turtle` `/data/trig` `/data/nq` `/data/rdfxml`（别名 `rdf-xml`/`rdf`） |
| `?format=` | `nt` `turtle` `trig` `nq` `rdfxml`/`rdf-xml`/`rdf` |
| Content-Type | `text/turtle`, `application/trig`, `application/n-triples`, `application/n-quads`, `application/rdf+xml` |

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
| `Authorization: Bearer <jwt>` | HS256 JWT（P5-02，可选替代） |

JWT Bearer 鉴权（P5-02，OIDC-ready 基线）：配置 `ONTOLITH_JWT_SECRET` 后，
`Authorization: Bearer <token>` 优先于 Header/API-Key 路径（未提供 Bearer 时回退 Header 鉴权）。
验证采用树内 HS256（RFC 7519 子集：base64url RFC 4648 §5 + SHA-256 FIPS 180-4 + HMAC-SHA256
RFC 2104，常量时间比对，RFC 4231/FIPS 180-4 向量背书）；`exp` 强制过期校验，
`ONTOLITH_JWT_ISSUER`/`ONTOLITH_JWT_AUDIENCE` 可选设置 `iss`/`aud` 精确匹配策略。
自定义 `tenant` claim 解析为租户（优先于 `X-Ontolith-Tenant` 传输头），`scope` claim
（空格分隔 `resource:action`）可覆盖默认权限，`sub` 为用户。`/health` 暴露 `jwt` 与 `tracing` 姿态（`on`/`off`）。

```bash
ONTOLITH_JWT_SECRET=...        # 启用 JWT Bearer 鉴权
ONTOLITH_JWT_ISSUER=ontolith   # 可选：iss 精确匹配
ONTOLITH_JWT_AUDIENCE=ontolith-server  # 可选：aud 精确匹配
```

#### 租户注册表（tenant registry）

配置/部署租户管理后（网关持注册表），Enforced 鉴权优先按 `X-API-Key` 的
FNV-1a 摘要解析注册表（`k:<digest>` 索引）：

- 命中注册表：租户 id 取自注册表；`X-Ontolith-Tenant` 若提供须与注册表一致
  （不一致 401）；`status=disabled` 拒绝；`X-Ontolith-User` 缺省为 `api`。
- 未命中：回退 legacy 路径——匹配全局 `ONTOLITH_API_KEY` + `X-Ontolith-Tenant`
  传输头（兼容既有部署）。
- `/health` 暴露 `tenants` 姿态（`on`/`off`），gRPC `HealthResponse` 同步字段。

#### OIDC 完整链路（R2+）

配置 `ONTOLITH_OIDC_JWKS_URL` 后，`Authorization: Bearer <token>` 走 OIDC/JWKS
验证路径（优先于共享密钥 HS256 路径）。树内实现（`crates/ontolith-security/
infrastructure/oidc.rs`，无第三方 JWT 依赖）：

- **JWKS/JWK（RFC 7517）**：`{"keys":[...]}` 解析，`kid`+`alg` 选键；RSA（RS256，
  RFC 7515 A.2.1 官方向量背书）与 oct（HS256）可用，EC/OKP 等键型在解析期被
  过滤——全部为不可用键时启动即报错（fail fast）。
- **签名验证**：RS256 = 自研无依赖大整数 RSA（PKCS#1 v1.5 + SHA-256，常量时间
  比对），HS256 = 复用树内 HMAC-SHA256。
- **claim 策略**：`exp`/`nbf` 强制校验（`ONTOLITH_JWT_LEEWAY_SECS` 允许时钟偏差）、
  `iss`/`aud` 精确匹配（`ONTOLITH_OIDC_ISSUER`/`ONTOLITH_OIDC_AUDIENCE`，未设时
  回退 `ONTOLITH_JWT_ISSUER`/`ONTOLITH_JWT_AUDIENCE`）；`tenant`/`scope`/`sub`
  映射与 HS256 路径一致。
- **密钥轮换**：`JwksVerifier` + `CachingJwks` 按 `ONTOLITH_OIDC_CACHE_TTL_SECS`
  （默认 300s）刷新；刷新失败继续服务上一组好键（离线容忍），无需重启。
- **发现文档（RFC 8414）**：`OidcDiscovery::parse` 强制 `issuer` 与配置一致
  （防 provider-confusion），库级可用。

JWKS 传输由 server 注入（`JwksFetcher`，与 L4 raft 同款最小 HTTP 栈）：

```bash
ONTOLITH_OIDC_JWKS_URL=file:///etc/ontolith/jwks.json   # 本地快照（演练/生产固定）
ONTOLITH_OIDC_JWKS_URL=http://idp.example:8080/jwks     # 启动抓取 + TTL 刷新
ONTOLITH_OIDC_CACHE_TTL_SECS=300                        # 可选：缓存 TTL（默认 300）
ONTOLITH_OIDC_ISSUER=https://idp.example                # 可选：iss 精确匹配
ONTOLITH_OIDC_AUDIENCE=ontolith-server                  # 可选：aud 精确匹配
ONTOLITH_JWT_LEEWAY_SECS=0                              # 可选：exp/nbf 时钟偏差
```

`https://` JWKS URL 在当前树内客户端下明确拒绝启动（TLS 客户端为后续项），
推荐经反向代理终结 TLS 或挂载 `file://` 快照。`/health` 暴露 `jwt`/`oidc` 姿态，
gRPC `HealthResponse` 同步 `oidc` 字段；启动日志打印 `jwt=… oidc=…` 姿态。
进程级演练固化在 [`scripts/drill-oidc-auth.sh`](../scripts/drill-oidc-auth.sh)
（真实 gateway 启动 + 有效 Bearer 200/`oidc:on` + 错误 issuer 401 + 伪造签名 401，
`=== OIDC DRILL PASS ===`）。

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
| ontolith-security | **30** | 鉴权/权限/审计（**SHA-256 哈希链完整性验证 + legacy FNV-1a 兼容续链**）+ `TenantMode`/`TenantNamespace` 命名空间校验 + **树内 HS256 JWT**（FIPS/RFC 4231 向量、sign/verify 往返、篡改/过期/iss/aud 拒绝、Bearer 鉴权）+ **OIDC 完整链路**（RFC 7515 A.2.1 RS256 官方向量、RFC 7517 JWKS 解析/kid 选键/不可用键过滤、发现文档 issuer 强制匹配、oct 往返 + 篡改/过期/iss/aud 拒绝、TTL 缓存刷新与坏响应保留旧钥）+ **租户注册表**（`Tenant`/`TenantApiKey`/`TenantStatus`、id 校验与 `system` 保留、确定性 JSON、`MemoryTenantStore`/`TenantService` create/update/delete/add_key/revoke_key、key 仅存摘要、摘要碰撞拒绝） |
| ontolith-server | **85** | turtle 写入、SPARQL JSON、tenant graph、强制鉴权、**RocksDB reopen**、**TLS 终止（rustls 往返）**、**R2 非 loopback TLS 门禁**、**强制租户隔离（acme/other 互不可见、越权引用 403、默认图写盖章）**、**JWT Bearer（认证、伪造/过期 401、JWT 租户优先盖章）**、**OIDC 链路（file:// 加载 + Bearer 认证往返、http:// JWKS 抓取、https 拒绝启动、`/health` jwt/oidc 姿态）**、**Tracing 全链路（`traceparent` 延续、根/子 span 父链、`Traceparent` 回带、`/admin/traces`）**、**gRPC 网关（roundtrip insert/select + `traceparent` 回带、enforced 401、跨租户 403、health+oidc）**、**租户管理（网关 CRUD + key 生命周期 + 鉴权即时生效 + digest 不泄漏、非 `system` 403、管理面代理 + ACL）**、**Ontology 载荷联动推理输入（P1-01：`reasoning_input_with_ontology` 合并 tbox/abox 角色图）**、**RDF/XML 入站/导出与 `/data` 导出（ttl/trig/nq/nt/rdf+xml 协商）、SPARQL 结果 SRX/TSV/CSV 协商、DESCRIBE HTTP** |

---

## 7. 已知限制

1. TLS 已落地（rustls 进程内终止 + R2 非 loopback 门禁）；HTTP/1.1 数据面尚无 HTTP/2；gRPC 网关为 HTTP/2（tonic，P5-01），完整框架中间件链仍无  
2. OIDC 完整链路已落地（JWKS + RS256/HS256 + claim 策略 + TTL 缓存刷新）；树内客户端仅支持 `file://`/`http://` JWKS，`https://` 需反向代理终结 TLS 或挂载快照（TLS 客户端为后续项）；RFC 8414 发现文档解析为库级能力，自动发现端点接线为后续项  
3. 审计哈希链已升级为 **SHA-256**（2026-08-29，P5-04：`sha256(prev‖payload)`，schema 不变；升级前 FNV-1a 64 文件按摘要长度判别兼容续链，`verify_chain()` 混合链全量校验）  
4. 租户隔离已升级为强制分库/行级（`ONTOLITH_TENANT_MODE=enforced`：命名图命名空间隔离 + 执行器租户视图）；分库物理隔离（每租户独立 RocksDB 实例）仍为后续增强  
5. SPARQL Results 输出已覆盖 JSON / XML(SRX) / TSV / CSV（SELECT/ASK）；CSV 为 RFC 4180 引号 + TSV 术语 cell 语义（IRI `<…>`、字面量 `@lang`/`^^<datatype>`、`_:` 标签），结果集输入侧解析不在范围内  

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
| 2026-08-29 | 2.11.0 | **审计加密级哈希升级（P5-04）**：链哈希 FNV-1a 64 → 树内 SHA-256（FIPS 180-4），`prev`/`hash` 新条目为 64 hex，legacy 16 hex 条目按摘要长度判别兼容验证并无缝续链；security 29→30 测（legacy 混合链测试）+ R3 门禁保持全绿 |

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
| 2026-08-08 | 2.6.0 | **P5-02 OIDC/JWT 鉴权基线**：树内 HS256 JWT 验证（RFC 7519 子集 + FIPS 180-4/RFC 4231 向量背书）+ `Authorization: Bearer` 接入（`ONTOLITH_JWT_SECRET`/`ISSUER`/`AUDIENCE`，`exp`/`iss`/`aud` 校验、JWT tenant claim 优先于传输头、`scope` 权限覆盖）；`/health` 暴露 `jwt` 姿态；security 12→18、server 24→26 测 |
| 2026-08-08 | 2.7.0 | **P5-05 Tracing 全链路**：`ontolith-observability` 追踪域模型 + `InMemoryTraceStore` + W3C `traceparent` 解析/生成 + 线程本地 `TraceScope`；server 网关 `http.request` 根 span + `http.auth`/`sparql.execute`/`data.ingest` 子 span、响应回带 `Traceparent`、`/health`+`/admin/config` 暴露 `tracing` 姿态、`GET /admin/traces`；observability 6→11、server 26→29 测 |
| 2026-08-08 | 2.8.0 | **P5-01 gRPC 网关接入**：tonic 0.12 + prost 0.13 + `protoc-bin-vendored` 3（`grpc-backend` feature 默认开，`--no-default-features` 回退构建通过）；`proto/ontolith/v1/sparql.proto` `SparqlService{Query,Health}`；`SparqlGateway` 复用 HTTP 共享执行路径 + metadata 鉴权（enforced 401/跨租户 403）+ `traceparent` 延续/回带 + 根/子 span；`serve_grpc` 独立 tokio runtime 线程；`ONTOLITH_GRPC_BIND`（默认 `127.0.0.1:50051`），`ontolith-server` bin 升级为真实 HTTP+gRPC 双网关；server 29→33 测 |
| 2026-08-08 | 2.9.0 | **OIDC 完整链路（R2+）**：`oidc.rs` 树内 JWKS/JWK（RFC 7517）+ RS256（RFC 7515 A.2.1 官方向量背书，自研无依赖大整数 RSA）+ HS256 + `exp`/`nbf`/`iss`/`aud` 策略 + RFC 8414 发现文档 issuer 强制匹配 + `JwksFetcher`/`CachingJwks`/`JwksVerifier` TTL 缓存刷新；server 接线 `ONTOLITH_OIDC_ISSUER`/`AUDIENCE`/`JWKS_URL`/`CACHE_TTL_SECS` + `ONTOLITH_JWT_LEEWAY_SECS`（file:///http:// 注入式传输，https 明确拒绝并文档化）；`/health`（HTTP+管理面）与 gRPC `HealthResponse` 暴露 `oidc` 姿态；security 18→24、server 44→49 测；R1 唯一剩余项勾选完成 |
| 2026-08-10 | 2.10.0 | **租户管理（注册表 CRUD + 管理面代理）**：`ontolith-security` 新增 `Tenant`/`TenantApiKey`/`TenantStatus` + `TenantStore`/`MemoryTenantStore`/`TenantService`（create/update/delete/add_key/revoke_key，key 仅存 FNV-1a 摘要、原始值一次性返回），Enforced 鉴权按 key 摘要解析注册表（头匹配/disabled 拒绝/user 缺省 `api`，全局 key 退化为 legacy 回退）；`ontolith-storage` 独立 `tenant` CF + `tenant_cf_*` 字节级原语；网关承载 `/admin/tenants*` CRUD（仅 `system` 租户 + cluster:admin，RocksDB 键布局 `t:<id>`/`k:<digest>`，`create_missing_column_families` 自动增 CF）；管理面 `/admin/tenants*` ACL 代理到网关（读 `authorize_admin_view`/写 `authorize_admin_mutation`，`http_exchange` 最小 HTTP 客户端）；`/health` 暴露 `tenants` 姿态；security 24→29、storage 51→54、server 49→65 测 |
| 2026-09-06 | 2.12.0 | **Jena/Fuseki 协议补齐（P0，HTTP 面）**：`GET /data` 数据集导出（全库或 enforced 租户 owned graphs；`?format=`/`Accept` 支持 `ttl`/`trig`/`nq`/`nt`/`rdf+xml`，Content-Type 对应）；ingest 支持 RDF/XML（`application/rdf+xml` Content-Type、`?format=rdf|rdfxml|rdf-xml|rdf/xml`、路径 `/data/rdfxml`/`/data/rdf-xml`/`/data/rdf`，路由补齐）；`/sparql` 结果协商 SRX（`application/sparql-results+xml`）/TSV/CSV（SELECT/ASK，`?format=`/`Accept`），JSON 默认兼容不变；SPARQL DESCRIBE HTTP 执行（描述图 JSON 与 CONSTRUCT 同构）；server 70→85 测 |
| 2026-09-06 | 2.13.0 | **远程 `LOAD <http://…>`（Jena/Fuseki 数据面缺口 Wave 2，HTTP 面）**：`rewrite_remote_loads` 在 plan 构建后把 `http(s)://` 源 `UpdateOp::Load` 原位改写为图感知 `INSERT DATA`——`crate::jsonld::http_get` 同步抓取（Content-Type + 状态码，仅 `http://`；`https://` 确定性 501/`SILENT` 跳过）→ Content-Type/URL 后缀探测（NT/NQ/Turtle/TriG/JSON-LD/RDF-XML）→ parser 解析 → 文档默认图载入 `INTO GRAPH` 目标（缺省为默认图）、文档命名图保留原名；请求内顺序与单事务原子性保持（改写后仍由引擎统一执行）；`SILENT` 抓取失败跳过、非 SILENT 请求级 500；enforced 租户下 `INTO GRAPH`/命名图经引擎 `validate_tenant_update_plan` 校验（越权 403）；server 85→91 测（6 个 e2e：named graph、default+N-Triples、TriG 命名图保留、错误/SILENT、https 501、enforced 403） |
| 2026-09-06 | 2.14.0 | **`/sparql` 查询侧 SPARQL 1.1 SERVICE 联邦（Jena/Fuseki 后续缺口 Wave 3，HTTP 面）**：query 引擎支持 `SERVICE [SILENT] (iri|?var) {…}`（常量/变量 endpoint；含 SERVICE 的 join/optional 右侧依赖式 dispatch；`SERVICE SILENT` 吞错；可序列化 IRI/字面量 VALUES 下推）；server 新增 `crate::federation::HttpServiceClient`——对远端执行 `SELECT * WHERE {…}?query=`（percent-encode）+ 共享 `crate::jsonld::http_get` 抓取（Content-Type 缺省按 SPARQL Results JSON），响应经 parser `sparql_results` 输入解析 + 本地字典 `_:label` 回填 blank；仅 `http://`，`https://` 经共享 http_get 确定性拒绝（501，后续轨）；`/sparql` 请求注入 `Arc<HttpServiceClient>`（绑定字典）；server 91→95 测（federation 单测 + HTTP e2e） |
| 2026-09-09 | 2.15.0 | **远程抓取 `https://` 落地（Jena/Fuseki 后续缺口 Wave 4，HTTP 面）**：`crate::jsonld::http_get` 新增 `https://` 分支——树内 rustls 0.23（ring provider）同步 TLS 客户端，信任锚为 `webpki-roots` Mozilla CA 集合 + 可选附加 PEM env `ONTOLITH_REMOTE_FETCH_CA_BUNDLE`（内部 PKI/自签端点）；三条出站路径统一受益：SPARQL Update 远程 `LOAD <https://…>`、`SERVICE <https://…>` 联邦、JSON-LD 远程 `@context`（`HttpRemoteContextLoader` 改走共享 `http_get`，删除重复的原裸 TCP 传输块）；响应解析抽出共享 `parse_http_response`；原「`https://` 确定性 501」e2e 改写为抓取失败语义（非 SILENT 500 / SILENT 跳过）；新增 6 测（rcgen 自签 TLS 服务往返、未信任证书/非 2xx 拒绝、env bundle 调度、loader https、非 http scheme 拒绝；env 测试模块级 ENV_LOCK 串行化、证书互不交叉），server 95→101 测；`webpki-roots` Tier B 登记；L3 2.21.0→2.22.0 同步 |
