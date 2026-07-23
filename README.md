# Ontolith

<p align="center">
  <img src="docs/images/ontolith-logo-04.png" alt="Ontolith Logo" width="220">
</p>

<p align="center">
  <a href="docs/Ontolith_Software_Architecture_Specification.md"><img src="https://img.shields.io/badge/Architecture-SAS--0001-1f6feb?style=for-the-badge" alt="Architecture"></a>
  <a href="docs/Ontolith_Development_Plan.md"><img src="https://img.shields.io/badge/Plan-R1--R4-0a7f42?style=for-the-badge" alt="Plan"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-green?style=for-the-badge" alt="License"></a>
</p>

**Rust-first semantic runtime and distributed knowledge graph platform.**

Ontolith is a layered Rust workspace for RDF data modeling, parsing, SPARQL query execution, storage and transaction kernels, cluster control, security, and observability.

---

## Quick Start

```bash
# 1) Build
cargo build --workspace

# 2) Run local gates
./scripts/ci-local.sh

# 3) Run compliance suites directly
cargo test -p ontolith-compliance --test sparql_r1_smoke -- --nocapture
cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture
```

### Runtime Reality Check

```bash
cargo run -p ontolith-server
```

The current `ontolith-server` binary executes bootstrap/runtime sampling and exits.
It does not yet stay as a long-running HTTP listener by default.

---

## What You Get

| Capability | Current Shape |
|------------|---------------|
| Semantic core | RDF/SPARQL-oriented layered runtime design |
| Query coverage | SPARQL smoke + curated W3C subset harness |
| Storage | In-memory engine + optional RocksDB backend |
| Gateway model | HTTP gateway implemented as library modules |
| Quality gates | fmt + clippy + workspace tests + compliance checks |

---

## HTTP Gateway Integration

The L5 gateway routes are implemented in the `ontolith-server` crate (`app` + `http`).
Included routes cover health, readiness, metrics, ingest, SPARQL, explain, audit, and cluster controls.

If you want a running HTTP listener today, wire `AppState` into `HttpServer` in your own launcher:

```rust
use ontolith_security::application::HeaderAuthenticator;
use ontolith_server::app::{shared_handler, AppState};
use ontolith_server::http::HttpServer;

fn main() -> std::io::Result<()> {
    let bind = "127.0.0.1:8080".to_string();
    let state = AppState::new_memory(bind.clone(), HeaderAuthenticator::default());
    let server = HttpServer::new(shared_handler(state));
    server.serve(&bind)
}
```

With that launcher running:

```bash
curl -s http://127.0.0.1:8080/health
curl -sG http://127.0.0.1:8080/sparql \
  --data-urlencode 'query=SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10'
```

When `ONTOLITH_AUTH_MODE=enforced`, include:

- `X-API-Key`
- `X-Ontolith-Tenant`
- `X-Ontolith-User`

---

## Crate Map

| Layer | Crate | Responsibility |
|-------|-------|----------------|
| L0 | `ontolith-core` | Identity/resource model, canonical encoding, shared errors |
| L1 | `ontolith-rdf` | Term/triple/quad/graph/dataset value model |
| L2 | `ontolith-storage` | Storage abstraction + in-memory/RocksDB adapters |
| L2 | `ontolith-transaction` | Transaction lifecycle and coordination |
| L3 | `ontolith-parser` | RDF parser layer |
| L3 | `ontolith-query` | SPARQL parse/optimize/execute pipeline |
| L4 | `ontolith-cluster` | Cluster consistency and control-plane primitives |
| L5 | `ontolith-server` | Access boundary and HTTP gateway implementation |
| Support | `ontolith-security` | Auth context, authorization, audit |
| Support | `ontolith-observability` | Metrics/tracing/logging model |
| Support | `ontolith-reasoner` | Reasoning extension surface |
| Support | `ontolith-plugin-api` | Plugin boundaries and contracts |
| Support | `ontolith-sdk` | SDK-facing integration surface |
| Quality | `ontolith-compliance` | SPARQL smoke and W3C subset test harness |

---

## Development Workflow

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Strict subset gate:

```bash
ONTOLITH_W3C_SUBSET_STRICT=1 ./scripts/ci-local.sh
```

---

## Deployment Notes

Deployment assets are provided under `deployments/` and `scripts/`.

```bash
# user-level service
cargo build -p ontolith-server --release
./scripts/install-ontolith-user-service.sh

# system-level service
cargo build -p ontolith-server --release
./scripts/install-ontolith-system-service.sh
```

Before enabling service units, verify your selected runtime entrypoint is a long-running listener.

---

## Documentation

| Document | Purpose |
|----------|---------|
| `docs/Ontolith_Software_Architecture_Specification.md` | Architecture baseline and constraints |
| `docs/Ontolith_Development_Plan.md` | Delivery planning and milestones |
| `docs/L0-ontolith-core-Knowledge-Object-Foundation.md` | L0 implementation notes |
| `docs/L1-ontolith-rdf-Statement-Graph-Dataset.md` | L1 implementation notes |
| `docs/L2-ontolith-storage-transaction-kernel.md` | L2 implementation notes |
| `docs/L3-ontolith-parser-query.md` | L3 implementation notes |
| `docs/L5-ontolith-access-security.md` | L5 API and security baseline |
| `docs/L5-systemd-service.md` | systemd operation guide |
| `adr/` | Architecture Decision Records |
| `rfc/` | Proposal and design drafts |

---

## Repository Layout

```text
ontolith/
├── crates/
├── docs/
├── rfc/
├── adr/
├── tests/
├── examples/
├── benchmarks/
└── deployments/
```

---

## Author

- Name: shark8848
- Email: admin@sharky-ai.com

## License

Apache-2.0. See `LICENSE` for full terms.
