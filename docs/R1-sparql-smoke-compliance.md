# R1 SPARQL Smoke Compliance Profile

文档 ID: COMP-R1-0001  
版本: 1.6.0  
日期: 2026-07-22  
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

Strict subset mode (no xfail / no skip):

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
- `unsupported`: documented and skipped for current scope.

Current known gaps:

- None in current v0 profile (remaining scope gaps are tracked as unsupported).

Current must-pass increment:

- Aggregate COUNT (no GROUP BY)
- Subquery (nested SELECT + LIMIT baseline)
- Property path sequence (iri/iri baseline)
- Property path `+` / `*` / `|` / `^` minimal set baseline

Current unsupported:

- Property path `?` operator and grouped/nested path forms beyond current minimal set
- SPARQL Update

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
- `sparql_w3c_subset_strict`: non-blocking observer (`ONTOLITH_W3C_SUBSET_STRICT=1`) for xfail/skip debt trend.

## Next

1. Expand subset from current seed to 20-40 cases with feature-tagged skip/xfail.
2. Keep required-lite stable for at least 3 CI samples, then evaluate strict promotion.
3. Move toward manifest-driven import of official W3C test artifacts.
