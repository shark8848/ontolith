#!/usr/bin/env bash
# R1 正式验收包 —— RDF 核心运行时可验收（PROGRESS.md "RDF 核心运行时可验收" 检查项）。
#   验收判据（全通过才算 ACCEPTANCE PASS）：
#     G1 静态门禁：cargo fmt --check + cargo clippy -D warnings
#     G2 全量测试：cargo test --workspace --all-targets（全部 ok，0 failed）
#     G3 标准符合性：w3c11_suite 492/492 PASS + shacl_suite 97/98 PASS（drift=0）
#     G4 运行时闭环：HTTP /sparql INSERT+SELECT roundtrip（内存后端，/health triples=1）
#     G5 持久化闭环：RocksDB 写 → 重启 reopen → 数据仍在（triples=1 + SELECT 可读）
#   用法：bash scripts/acceptance-r1.sh [--skip-workspace-tests]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SKIP_WS="${1:-}"
EVID="${ACCEPTANCE_EVIDENCE_DIR:-/tmp/ontolith-r1-acceptance-$$}"
mkdir -p "$EVID"
SUMMARY="$EVID/acceptance-summary.txt"
: > "$SUMMARY"
PORT=$((18090 + (RANDOM % 400)))
BIN="$ROOT/target/debug/ontolith-server"

log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$SUMMARY"; }
fail() { log "FAIL: $*"; exit 1; }
pass() { log "PASS: $*"; }
field() { echo "$1" | grep -o "\"$2\":[^,}]*" | head -1 | sed 's/^[^:]*://' | tr -d '"' || true; }

SPARQL_PID=""
stop_srv() { [[ -n "$SPARQL_PID" ]] && { kill "$SPARQL_PID" >/dev/null 2>&1 || true; wait "$SPARQL_PID" 2>/dev/null || true; }; SPARQL_PID=""; }
trap 'stop_srv' EXIT

start_srv() { # $1=storage $2=data_dir $3=tag
  stop_srv
  ONTOLITH_CLUSTER_MODE=memory \
  ONTOLITH_BIND="127.0.0.1:${PORT}" \
  ONTOLITH_STORAGE="$1" \
  ONTOLITH_DATA_DIR="$2" \
  "$BIN" >"$EVID/server-${3}.log" 2>&1 &
  SPARQL_PID=$!
  local i
  for i in $(seq 1 30); do
    curl -fsS --max-time 2 "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1 && return 0
    sleep 1
  done
  tail -20 "$EVID/server-${3}.log" >&2 || true
  fail "server did not become healthy (${3})"
}

srv_triples() { field "$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT}/health")" triples; }

log "=== R1 RDF core runtime acceptance ==="
log "evidence dir: $EVID  server port: $PORT"

# ---- G1 static gates ----
log "G1: cargo fmt --check"
cargo fmt --all -- --check 2>&1 | tail -2 >>"$SUMMARY"
pass "fmt --check"

log "G1: cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2 >>"$SUMMARY"
pass "clippy -D warnings"

# ---- G2 full workspace tests ----
if [[ "$SKIP_WS" != "--skip-workspace-tests" ]]; then
  log "G2: cargo test --workspace --all-targets (log: workspace-tests.log)"
  cargo test --workspace --all-targets 2>&1 >"$EVID/workspace-tests.log" || fail "workspace tests failed"
  ws_failed="$(grep -c 'test result: FAILED' "$EVID/workspace-tests.log" || true)"
  ws_ok="$(grep -c 'test result: ok' "$EVID/workspace-tests.log" || true)"
  (( ws_failed == 0 )) || fail "workspace tests: ${ws_failed} FAILED result line(s)"
  pass "workspace tests: ${ws_ok} 'test result: ok' lines, 0 failed"
else
  log "G2: skipped (--skip-workspace-tests)"
fi

# ---- G3 standard conformance ----
log "G3: w3c11_suite (expect total=492 pass=492 fail=0 drift=0)"
cargo test -p ontolith-compliance --test w3c11_suite -- --nocapture >"$EVID/w3c11.log" 2>&1 || fail "w3c11_suite failed"
grep -q '\[w3c11 summary\] total=492 pass(must-pass)=492 fail(known-gap)=0 drift=0 missing=0' "$EVID/w3c11.log" \
  || fail "w3c11_suite summary mismatch (see w3c11.log: $(grep '\[w3c11 summary\]' "$EVID/w3c11.log" | tail -1 || true))"
pass "w3c11_suite 492/492, drift=0"

log "G3: shacl_suite (expect total=98 pass=97 fail=1 drift=0)"
cargo test -p ontolith-compliance --test shacl_suite -- --nocapture >"$EVID/shacl.log" 2>&1 || fail "shacl_suite failed"
grep -q '\[shacl summary\] total=98 pass(must-pass)=97 fail(known-gap)=1 drift=0 missing=0' "$EVID/shacl.log" \
  || fail "shacl_suite summary mismatch (see shacl.log: $(grep '\[shacl summary\]' "$EVID/shacl.log" | tail -1 || true))"
pass "shacl_suite 97/98, drift=0"

# ---- G4 runtime roundtrip (memory) ----
log "G4: build server + HTTP /sparql INSERT+SELECT roundtrip (memory)"
cargo build -p ontolith-server --bin ontolith-server 2>&1 | tail -2 >>"$SUMMARY"
start_srv memory "" memory
ins="$(curl -fsS --max-time 5 -X POST -H 'Content-Type: application/sparql-query' \
  --data-binary 'INSERT DATA { <urn:acc:s1> <urn:acc:p> "v1" }' \
  "http://127.0.0.1:${PORT}/sparql")"
echo "$ins" | grep -q '"affected":1' || fail "INSERT DATA not acknowledged: ${ins}"
sel="$(curl -fsS --max-time 5 -X POST -H 'Content-Type: application/sparql-query' \
  --data-binary 'SELECT ?s WHERE { ?s <urn:acc:p> "v1" }' \
  "http://127.0.0.1:${PORT}/sparql")"
echo "$sel" | grep -q 'urn:acc:s1' || fail "SELECT did not return inserted subject: ${sel}"
n="$(srv_triples)"
(( n == 1 )) || fail "memory backend /health triples != 1 (got ${n})"
stop_srv
pass "memory roundtrip: INSERT affected=1, SELECT returns urn:acc:s1, /health triples=1"

# ---- G5 rocksdb reopen persistence ----
log "G5: RocksDB write -> restart reopen -> data intact"
DATA="$EVID/data"
start_srv rocksdb "$DATA" rocksdb
curl -fsS --max-time 5 -X POST -H 'Content-Type: application/sparql-query' \
  --data-binary 'INSERT DATA { <urn:acc:r1> <urn:acc:p> "persist" }' \
  "http://127.0.0.1:${PORT}/sparql" | grep -q '"affected":1' || fail "rocksdb INSERT failed"
(( $(srv_triples) == 1 )) || fail "rocksdb /health triples != 1 after INSERT"
stop_srv
start_srv rocksdb "$DATA" rocksdb-reopen
(( $(srv_triples) == 1 )) || fail "data lost after reopen (triples=$(srv_triples))"
sel2="$(curl -fsS --max-time 5 -X POST -H 'Content-Type: application/sparql-query' \
  --data-binary 'SELECT ?s WHERE { ?s <urn:acc:p> "persist" }' \
  "http://127.0.0.1:${PORT}/sparql")"
echo "$sel2" | grep -q 'urn:acc:r1' || fail "reopened DB SELECT missed data: ${sel2}"
stop_srv
pass "rocksdb reopen: /health triples=1 after restart, SELECT returns urn:acc:r1"

log "=== ACCEPTANCE PASS ==="
log "summary: $SUMMARY"
