# R1 SPARQL Smoke Compliance Profile

文档 ID: COMP-R1-0001  
版本: 1.7.1  
日期: 2026-07-23  
Crate: [`ontolith-compliance`](../crates/ontolith-compliance)

---

## Purpose

Pin a **curated smoke profile** for the R1 SPARQL query baseline, and run a
**W3C-inspired subset harness** as an exploratory gate. This is still **not**
the full W3C SPARQL 1.1 official suite.

## How to run

```bash
cargo test -p ontolith-compliance --test sparql_r1_smoke
cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture
# or full local CI:
./scripts/ci-local.sh
```

Strict subset mode (no xfail / no in-scope skip):

```bash
ONTOLITH_W3C_SUBSET_STRICT=1 ./scripts/ci-local.sh
# backward-compatible alias:
ONTOLITH_W3C_SUBSET_REQUIRED=1 ./scripts/ci-local.sh
```

## Covered features

See `ontolith_compliance::SPARQL_R1_SMOKE_FEATURES`:

- SELECT / ASK / CONSTRUCT
- BGP JOIN, OPTIONAL, UNION
- FILTER, BIND, VALUES
- PREFIX, DISTINCT, ORDER BY, LIMIT/OFFSET
- N-Triples / Turtle parse → query

## W3C subset profile (v0)

Location: `crates/ontolith-compliance/tests/w3c/`

Classification:

- `must-pass`: blocks strict gate; must stay green.
- `known-gap`: executed and expected to fail until feature lands.
- `unsupported`: documented and skipped for current scope; may be strict skip-exempt when explicitly marked out-of-scope.

Current known gaps:

- None in current v0 profile (remaining scope gaps are tracked as unsupported).

Current must-pass increment:

- Aggregate COUNT (no GROUP BY)
- Aggregate COUNT(*)
- Subquery (nested SELECT + LIMIT baseline)
- Property path sequence (iri/iri baseline)
- Property path `+` / `*` / `|` / `^` minimal set baseline
- ASK false / fixed-subject join / VALUES tuple / DISTINCT+OFFSET variants

Current unsupported:

- Property path `?` operator and grouped/nested path forms beyond current minimal set
- SPARQL Update (strict skip-exempt)

## Out of scope (R1+)

- Property path `?` operator and grouped/nested path forms beyond current minimal set
- Advanced subquery forms beyond nested SELECT + LIMIT baseline
- Full aggregates/GROUP BY/HAVING beyond COUNT baseline
- SPARQL Update
- Full W3C manifest-driven suite
- Performance / SLO gates

## CI gating mode

- `sparql_r1_smoke`: required (blocking).
- `sparql_w3c_subset`: required-lite (blocking must-pass regressions).
- `sparql_w3c_subset_strict`: non-blocking observer (`ONTOLITH_W3C_SUBSET_STRICT=1`) for xfail and in-scope skip debt trend.
- `sparql_w3c_strict_promotion_readiness`: main 分支自动评估最近 3 次 strict observer 是否连续全绿，并在 Job Summary 输出 READY/NOT READY 信号。

Current profile snapshot (v0, 2026-07-23):

- must-pass: 24
- known-gap: 0
- unsupported: 1 (SPARQL Update, strict skip-exempt)

## Next

1. Keep subset within 20-40 cases while preserving explicit expected assertions per case.
2. Require 3 consecutive `main` CI green runs (including strict observer pass) before promoting strict to required; readiness 由 CI 自动汇总信号提供。
3. Move toward manifest-driven import of official W3C test artifacts.
