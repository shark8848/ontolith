# W3C SPARQL 1.1 Test Suite (vendored, manifest-driven)

This directory vendors the official
[W3C RDF Tests / SPARQL 1.1 suite](https://github.com/w3c/rdf-tests)
(query evaluation, update evaluation, and syntax manifests) so the compliance
harness runs the **real** official cases instead of a hand-picked subset.

- Source: `w3c/rdf-tests` → `sparql/sparql11` (aggregates, property-path,
  bindings, construct, negation, subquery, functions, syntax, update, …).
- Scope: 941 files / ~4 MB across 28 feature directories.
- License: W3C Software and Document License (see the `w3c/rdf-tests` repo).

## Features included

`add`, `aggregates`, `basic-update`, `bind`, `bindings`, `cast`, `clear`,
`construct`, `copy`, `csv-tsv-res`, `delete`, `delete-data`, `delete-insert`,
`delete-where`, `drop`, `exists`, `functions`, `grouping`, `json-res`, `move`,
`negation`, `project-expression`, `property-path`, `subquery`, `syntax-query`,
`syntax-update-1`, `syntax-update-2`, `update-silent`.

## Runner

`crates/ontolith-compliance/tests/w3c11_suite.rs` is a manifest-driven runner:

1. Discovers every feature `manifest.ttl` and parses it with our own Turtle
   parser (RDF lists, nested `mf:action`, named-graph data references).
2. Executes `QueryEvaluationTest`, `UpdateEvaluationTest`,
   `PositiveSyntaxTest(11)` and `NegativeSyntaxTest(11)` cases.
3. Compares against official expected results: SRX / SRJ / TSV / CSV, Turtle
   graph isomorphism, and ASK booleans.
4. Locks every outcome in `../w3c11_profile.tsv` (`feature\tname\tPASS|FAIL\t
   reason-code`) so known gaps are documented and regressions fail CI.

Run:

```bash
# Profile-locked gate (normal mode; fails on any drift vs w3c11_profile.tsv)
cargo test -p ontolith-compliance --test w3c11_suite

# Regenerate the profile after implementing a feature
ONTOLITH_W3C11_LEARN=1 cargo test -p ontolith-compliance --test w3c11_suite
```

## Current baseline

492 manifest cases: **127 PASS / 365 FAIL** (failures grouped by reason:
`parse-error` 223, `data-format` 52, `semantic` 48, `accepted-invalid` 17,
`named-graph` 16, `other` 9). The profile is the compliance backlog: each
FAIL row is a documented gap; implement a feature, regenerate the profile, and
the gate stays green while the PASS count grows.
