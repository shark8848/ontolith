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

  w3c_subset_strict_mode="${ONTOLITH_W3C_SUBSET_STRICT:-0}"
  if [[ "${ONTOLITH_W3C_SUBSET_REQUIRED:-0}" == "1" && "${w3c_subset_strict_mode}" != "1" ]]; then
    # Backward-compatible alias from earlier script behavior.
    echo "INFO: ONTOLITH_W3C_SUBSET_REQUIRED=1 is treated as strict mode; prefer ONTOLITH_W3C_SUBSET_STRICT=1."
    w3c_subset_strict_mode="1"
  fi

  if [[ "${w3c_subset_strict_mode}" == "1" ]]; then
    echo "==> SPARQL W3C subset (strict mode)"
    ONTOLITH_W3C_SUBSET_STRICT=1 \
      cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture
  else
    echo "==> SPARQL W3C subset (required-lite mode)"
    cargo test -p ontolith-compliance --test sparql_w3c_subset -- --nocapture
  fi
fi

echo "==> OK: local CI gates passed"
