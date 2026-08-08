#!/usr/bin/env bash
# P7-03 actual release + rollback drill (staging, no production data touched).
#   V_new  = HEAD (9fac343, current main)
#   V_prev = ec1d539 (L6 收尾, previous feature release; binary differs from V_new)
# Stages: release deploy+verify -> code-level rollback+verify -> data-level
#         rollback (backup/move-aside/restore)+verify -> restore V_new -> rebuild.
set -euo pipefail

ROOT=/home/ontolith
VNEW_BIN="$ROOT/target/release"
VNEW_SHA=$(git -C "$ROOT" rev-parse --short HEAD)
VPREV_SHA=ec1d539

STAGE="${ONTOLITH_DRILL_STAGE:-/tmp/ontolith-release-drill-$$}"
mkdir -p "$STAGE/bin" "$STAGE/bin-vnew" "$STAGE/bin-vprev" "$STAGE/data" "$STAGE/backup" "$STAGE/logs" "$STAGE/evidence" "$STAGE/src-vprev"
EVID="$STAGE/evidence/release-rollback-transcript.txt"
: > "$EVID"

PORT_G=$((18090 + (RANDOM % 400)))
PORT_M=$((19091 + (RANDOM % 400)))

log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$EVID"; }
fail() { log "FAIL: $*"; exit 1; }
ok() { log "OK: $*"; }
field() { echo "$1" | grep -o "\"$2\":[^,}]*" | head -1 | sed 's/^[^:]*://' | tr -d '"' || true; }

GW_PID=""; MG_PID=""
stop_all() {
  for pid in "$GW_PID" "$MG_PID"; do
    if [[ -n "$pid" ]]; then kill "$pid" >/dev/null 2>&1 || true; wait "$pid" 2>/dev/null || true; fi
  done
  GW_PID=""; MG_PID=""
}
trap stop_all EXIT

start_services() {
  local tag="$1"
  stop_all
  ONTOLITH_CLUSTER_MODE=memory \
  ONTOLITH_BIND="127.0.0.1:${PORT_G}" \
  ONTOLITH_STORAGE=rocksdb \
  ONTOLITH_DATA_DIR="$STAGE/data" \
  "$STAGE/bin/ontolith-server" >"$STAGE/logs/gateway-${tag}.log" 2>&1 &
  GW_PID=$!

  ONTOLITH_CLUSTER_MODE=memory \
  ONTOLITH_MANAGEMENT_BIND="127.0.0.1:${PORT_M}" \
  ONTOLITH_BIND="127.0.0.1:${PORT_G}" \
  ONTOLITH_STORAGE=memory \
  "$STAGE/bin/ontolith-management-server" >"$STAGE/logs/management-${tag}.log" 2>&1 &
  MG_PID=$!

  local i
  for i in $(seq 1 60); do
    if curl -fsS --max-time 2 "http://127.0.0.1:${PORT_G}/health" >/dev/null 2>&1 \
       && curl -fsS --max-time 2 "http://127.0.0.1:${PORT_M}/admin/health" >/dev/null 2>&1; then
      ok "services up (${tag}): gateway=${PORT_G} management=${PORT_M}"
      return 0
    fi
    sleep 1
  done
  log "gateway log:"; tail -20 "$STAGE/logs/gateway-${tag}.log" || true
  log "management log:"; tail -20 "$STAGE/logs/management-${tag}.log" || true
  fail "services did not become healthy (${tag})"
}

verify_health() {
  local tag="$1"
  local gwh mg mon hlat
  gwh="$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT_G}/health")" || fail "gateway /health failed (${tag})"
  mg="$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT_M}/admin/health")" || fail "management /admin/health failed (${tag})"
  mon="$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT_M}/admin/monitoring")" || fail "monitoring failed (${tag})"
  hlat="$(field "$mon" latency_ms)"
  [[ "$(field "$mon" reachable)" == "true" ]] || fail "runtime_probe unreachable (${tag})"
  ok "health (${tag}): gateway_ok mgmt_ok probe.latency_ms=${hlat:-null}"
}

gw_triples() { field "$(curl -fsS --max-time 3 "http://127.0.0.1:${PORT_G}/health")" triples; }

ingest_triple() {
  curl -fsS --max-time 5 -X POST \
    -H 'Content-Type: application/sparql-query' \
    --data-binary 'INSERT DATA { <urn:drill:s1> <urn:drill:p> "v1" }' \
    "http://127.0.0.1:${PORT_G}/sparql" >/dev/null || fail "SPARQL INSERT failed"
}

log "=== Ontolith P7-03 release/rollback drill ==="
log "V_new=${VNEW_SHA} V_prev=${VPREV_SHA} stage=${STAGE} ports=${PORT_G}/${PORT_M}"

# ---------- build V_new (current release) ----------
log "building release binaries (HEAD ${VNEW_SHA}) ..."
# shared CARGO_TARGET_DIR freshness check: force rebuild so a stale binary
# left by a previous drill cannot be mistaken for the current HEAD release
find "$ROOT/crates" "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" -type f -exec touch {} + 2>/dev/null || true
( cd "$ROOT" && cargo build --release -p ontolith-server --bin ontolith-server --bin ontolith-management-server 2>&1 | tail -2 ) >>"$EVID"
cp "$VNEW_BIN/ontolith-server" "$VNEW_BIN/ontolith-management-server" "$STAGE/bin-vnew/"
n="$(strings "$STAGE/bin-vnew/ontolith-management-server" | grep -c shard_map_epoch || true)"
(( n > 0 )) || fail "V_new binary missing L7 field string (string count=${n})"
ok "V_new release binaries built (L7 fields present, string count=${n})"

# ---------- 1. release deploy ----------
cp "$STAGE/bin-vnew/ontolith-server" "$STAGE/bin-vnew/ontolith-management-server" "$STAGE/bin/"
start_services vnew
verify_health vnew

# ---------- 2. ingest + verify data ----------
ingest_triple
n="$(gw_triples)"
(( n == 1 )) || fail "expected 1 triple after INSERT, got ${n}"
ok "ingest + query roundtrip: ${n} triple(s) in RocksDB data dir"
stop_all
cp -a "$STAGE/data" "$STAGE/backup/data-before-rollback"
ok "quiesced backup of data dir -> backup/data-before-rollback"
start_services vnew-backup-check
verify_health vnew-backup-check
n="$(gw_triples)"
(( n == 1 )) || fail "triple lost after restart, got ${n}"
ok "data survives restart (persistence check)"

# ---------- 3. code-level rollback to V_prev (ec1d539) ----------
log "building rollback release binaries (${VPREV_SHA}) ..."
( cd "$ROOT" && git archive "$VPREV_SHA" | tar -x -C "$STAGE/src-vprev" )
# force cargo to rebuild: shared CARGO_TARGET_DIR fingerprints would otherwise
# treat the freshly-extracted tree as fresh (mtime-based freshness check)
find "$STAGE/src-vprev" -type f -exec touch {} +
( cd "$STAGE/src-vprev" && CARGO_TARGET_DIR="$ROOT/target" cargo build --release -p ontolith-server --bin ontolith-server --bin ontolith-management-server 2>&1 | tail -2 ) >>"$EVID"
cp "$ROOT/target/release/ontolith-server" "$ROOT/target/release/ontolith-management-server" "$STAGE/bin-vprev/"
n="$(strings "$STAGE/bin-vprev/ontolith-management-server" | grep -c shard_map_epoch || true)"
(( n == 0 )) || fail "V_prev binary still has L7 field string (string count=${n})"
ok "V_prev release binaries built (L7 fields absent, string count=${n})"
stop_all
cp "$STAGE/bin-vprev/ontolith-server" "$STAGE/bin-vprev/ontolith-management-server" "$STAGE/bin/"
start_services vprev
verify_health vprev
n="$(gw_triples)"
(( n == 1 )) || fail "data not intact after code rollback (expected 1, got ${n})"
ok "code-level rollback: data dir untouched, ${n} triple(s) still queryable on V_prev binary"

# ---------- 4. data-level rollback (backup -> simulate loss -> restore) ----------
stop_all
mv "$STAGE/data" "$STAGE/data-sim-corrupt"
start_services vprev-data-lost
verify_health vprev-data-lost
n="$(gw_triples)"
(( n == 0 )) || fail "expected empty DB after data loss, got ${n}"
ok "simulated data loss: fresh DB reports ${n} triples"
stop_all
rm -rf "$STAGE/data"
cp -a "$STAGE/backup/data-before-rollback" "$STAGE/data"
start_services vprev-data-restored
verify_health vprev-data-restored
n="$(gw_triples)"
(( n == 1 )) || fail "data not recovered from backup (expected 1, got ${n})"
ok "data-level rollback: restored from backup, ${n} triple(s) recovered"

# ---------- 5. restore V_new ----------
stop_all
cp "$STAGE/bin-vnew/ontolith-server" "$STAGE/bin-vnew/ontolith-management-server" "$STAGE/bin/"
start_services vnew-restored
verify_health vnew-restored
n="$(gw_triples)"
(( n == 1 )) || fail "data not intact after restore (expected 1, got ${n})"
ok "restored V_new (${VNEW_SHA}): ${n} triple(s) intact"

stop_all
log "=== DRILL PASS ==="
log "evidence: $EVID"
log "stage: $STAGE"

# leave the machine on the current release (rebuild HEAD into target/release)
log "rebuilding current release (HEAD ${VNEW_SHA}) into target/release ..."
find "$ROOT/crates" "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" -type f -exec touch {} + 2>/dev/null || true
( cd "$ROOT" && cargo build --release -p ontolith-server --bin ontolith-server --bin ontolith-management-server 2>&1 | tail -2 ) >>"$EVID"
n="$(strings "$ROOT/target/release/ontolith-management-server" | grep -c shard_map_epoch || true)"
(( n > 0 )) || fail "target/release not restored to HEAD (string count=${n})"
ok "target/release restored to ${VNEW_SHA} (verified by binary string, count=${n})"
