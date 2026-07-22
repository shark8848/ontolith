#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml (no Docker).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace --all-targets

if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q '"ontolith-compliance"'; then
  echo "==> SPARQL R1 smoke (ontolith-compliance)"
  cargo test -p ontolith-compliance --test sparql_r1_smoke -- --nocapture

  if [[ "${ONTOLITH_W3C_SUBSET_REQUIRED:-0}" == "1" ]]; then
    echo "==> SPARQL W3C subset (required mode)"
    ONTOLITH_W3C_SUBSET_STRICT=1 \
      cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture
  else
    echo "==> SPARQL W3C subset (non-blocking mode)"
    if ! cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture; then
      echo "WARN: W3C subset failed in non-blocking mode; set ONTOLITH_W3C_SUBSET_REQUIRED=1 to enforce."
    fi
  fi
fi

echo "==> OK: local CI gates passed"
