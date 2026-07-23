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

echo "==> management server smoke"
cargo build -p ontolith-server --bin ontolith-management-server
ONTOLITH_MANAGEMENT_BIND=127.0.0.1:19091 \
ONTOLITH_BIND=127.0.0.1:18080 \
./target/debug/ontolith-management-server >/tmp/ontolith-management-smoke.log 2>&1 &
mgmt_pid=$!
trap 'kill "$mgmt_pid" >/dev/null 2>&1 || true' EXIT

timeout 20s bash -c 'until curl -fsS "http://127.0.0.1:19091/admin/health" >/dev/null 2>&1; do :; done' || {
  echo "management smoke timeout; server log:"
  tail -n 80 /tmp/ontolith-management-smoke.log || true
  exit 1
}
curl -fsS "http://127.0.0.1:19091/admin/monitoring" | grep -q '"runtime_probe"'

kill "$mgmt_pid" >/dev/null 2>&1 || true
wait "$mgmt_pid" 2>/dev/null || true
trap - EXIT

echo "==> OK: local CI gates passed"
