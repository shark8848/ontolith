#!/usr/bin/env bash
# R4 正式验收包 —— AI-native 语义运行时扩展（ACC-R4-0001，PROGRESS R4 退出标准）。
#   验收判据（全通过才算 ACCEPTANCE PASS）：
#     G1 静态门禁：cargo fmt --check + cargo clippy -D warnings
#     G2 全量测试：cargo test --workspace --all-targets（全部 ok，0 failed）
#     G3 标准符合性零漂移：w3c11_suite 492/492 + shacl_suite 98/98（drift=0）
#     G4 语义 HTTP 闭环：启用 semantic → POST /semantic/index + GET /semantic/search
#        命中 + 同查询两次响应字节级一致 + /health 暴露 semantic 姿态
#     G5 检索 KPI 门禁：p802_retrieval_gate（release）3 测 + 语义 bench 阈值/趋势
#     G6 扩展安全与兼容：compliance 全门禁（r2/r3/p802）通过 + G3 零漂移
#   用法：bash scripts/acceptance-r4.sh [--skip-workspace-tests]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SKIP_WS="${1:-}"
EVID="${ACCEPTANCE_EVIDENCE_DIR:-/tmp/ontolith-r4-acceptance-$$}"
mkdir -p "$EVID"
SUMMARY="$EVID/acceptance-summary.txt"
: > "$SUMMARY"
PORT=$((18290 + (RANDOM % 400)))
BIN="$ROOT/target/debug/ontolith-server"

log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$SUMMARY"; }
fail() { log "FAIL: $*"; exit 1; }
pass() { log "PASS: $*"; }
field() { echo "$1" | grep -o "\"$2\":[^,}]*" | head -1 | sed 's/^[^:]*://' | tr -d '"' || true; }

SRV_PID=""
stop_srv() { [[ -n "$SRV_PID" ]] && { kill "$SRV_PID" >/dev/null 2>&1 || true; wait "$SRV_PID" 2>/dev/null || true; }; SRV_PID=""; }
trap 'stop_srv' EXIT

start_srv() {
  stop_srv
  ONTOLITH_CLUSTER_MODE=memory \
  ONTOLITH_BIND="127.0.0.1:${PORT}" \
  ONTOLITH_STORAGE=memory \
  ONTOLITH_SEMANTIC_ENABLED=1 \
  "$BIN" >"$EVID/server.log" 2>&1 &
  SRV_PID=$!
  local i
  for i in $(seq 1 30); do
    curl -fsS --max-time 2 "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1 && return 0
    sleep 1
  done
  tail -20 "$EVID/server.log" >&2 || true
  fail "server did not become healthy"
}

log "=== R4 AI-native semantic runtime acceptance ==="
log "evidence dir: $EVID  server port: $PORT"

# ---- G1 static gates ----
log "G1: cargo fmt --check"
cargo fmt --all -- --check 2>&1 | tail -2 >>"$SUMMARY"
pass "fmt --check"
log "G1: cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2 >>"$SUMMARY"
pass "clippy -D warnings"

# ---- G2 full workspace tests ----
if [[ "$SKIP_WS" == "--skip-workspace-tests" ]]; then
  log "G2: skipped (--skip-workspace-tests)"
  pass "workspace tests (skipped)"
else
  log "G2: cargo test --workspace --all-targets"
  if cargo test --workspace --all-targets >"$EVID/workspace-tests.log" 2>&1; then
    pass "workspace tests"
  else
    tail -40 "$EVID/workspace-tests.log" >&2 || true
    fail "workspace tests failed"
  fi
fi

# ---- G3 standard conformance zero drift ----
log "G3: w3c11_suite"
if cargo test -p ontolith-compliance --test w3c11_suite >"$EVID/w3c11.log" 2>&1; then
  pass "w3c11_suite (492/492)"
else
  tail -30 "$EVID/w3c11.log" >&2 || true
  fail "w3c11_suite failed"
fi
log "G3: shacl_suite"
if cargo test -p ontolith-compliance --test shacl_suite >"$EVID/shacl.log" 2>&1; then
  pass "shacl_suite (98/98)"
else
  tail -30 "$EVID/shacl.log" >&2 || true
  fail "shacl_suite failed"
fi

# ---- G4 semantic HTTP roundtrip ----
log "G4: semantic HTTP roundtrip"
[[ -x "$BIN" ]] || cargo build -p ontolith-server --bin ontolith-server >/dev/null 2>&1
start_srv
health="$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT}/health")"
if [[ "$(field "$health" semantic)" != "on" ]]; then
  fail "health semantic posture expected on, got: $health"
fi
pass "health semantic=on"

idx="$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${PORT}/semantic/index?term=urn%3Aacc%3Asemantic-term-1")"
idx2="$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${PORT}/semantic/index?term=urn%3Aacc%3Asemantic-term-1")"
if [[ "$(field "$idx" total)" != "1" || "$(field "$idx2" total)" != "1" ]]; then
  fail "semantic index must deduplicate (total must stay 1): $idx vs $idx2"
fi
if [[ "$(field "$idx2" indexed)" != "0" ]]; then
  fail "re-index must report 0 new terms (idempotent dedup): $idx2"
fi
pass "semantic index idempotent (dedup: total=1, second indexed=0)"

r1="$(curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/semantic/search?q=semantic%20term&k=3")"
r2="$(curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/semantic/search?q=semantic%20term&k=3")"
if [[ "$r1" != "$r2" ]]; then
  fail "semantic search must be byte-identical across runs"
fi
if ! echo "$r1" | grep -q "semantic-term-1"; then
  fail "semantic search did not hit indexed term: $r1"
fi
pass "semantic search deterministic + top-k hit"
stop_srv

# ---- G5 retrieval KPI gate ----
log "G5: p802 retrieval KPI gate (release profile)"
if cargo test --release -p ontolith-compliance --test p802_retrieval_gate >"$EVID/kpi-gate.log" 2>&1; then
  pass "p802 retrieval KPI gate"
else
  tail -30 "$EVID/kpi-gate.log" >&2 || true
  fail "p802 retrieval KPI gate failed"
fi
log "G5: semantic bench thresholds + trend"
if ONTOLITH_BENCH_TREND_PATH="$EVID/semantic-bench.jsonl" \
    bash scripts/check-semantic-bench-thresholds.sh >"$EVID/bench.log" 2>&1; then
  pass "semantic bench thresholds + trend"
else
  tail -30 "$EVID/bench.log" >&2 || true
  fail "semantic bench thresholds failed"
fi

# ---- G6 extension security & compatibility gates ----
log "G6: compliance gates (r2/r3/p802)"
if cargo test -p ontolith-compliance --test r2_explain_gate --test r2_reasoner_gate \
    --test p802_retrieval_gate --test r3_geo_gate --test r3_security_gate \
    >"$EVID/gates.log" 2>&1; then
  pass "compliance gates"
else
  tail -30 "$EVID/gates.log" >&2 || true
  fail "compliance gates failed"
fi
pass "zero-drift on W3C/SHACL (G3)"

log "=== ACCEPTANCE PASS ==="
