#!/usr/bin/env bash
# P7-01 / P7-04: online rebalance + disaster recovery drill.
#
# Boots a real 3-node multi-process raft cluster (openraft HTTP RPC +
# RocksDB raft CF, same paths as the CI smoke) and walks through:
#   1. election        - a leader is elected
#   2. online rebalance - POST /admin/data/rebalance moves real slots when the
#      boot shard map is deliberately skewed via ONTOLITH_RAFT_SLOT_BIAS
#   3. replication     - appended entries commit on every node
#   4. DR: follower loss - a follower is killed; majority commit keeps advancing
#   5. DR: leader loss   - the leader is killed; a new leader is elected and
#      commits continue (automatic failover)
#   6. DR: restart/rejoin - the killed node restarts on its original storage
#      path and catches up to the current commit index
#
# Evidence: every step is recorded into $ONTOLITH_DRILL_EVIDENCE_DIR
# (default: $TMPDIR/ontolith-drill-<pid>/evidence/drill-transcript.txt) with
# timestamps and observed metrics; the script exits non-zero on any failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="${ONTOLITH_DRILL_BIN:-$ROOT/target/debug/ontolith-management-server}"
NODES="${ONTOLITH_DRILL_NODES:-3}"
BASE_PORT="${ONTOLITH_DRILL_BASE_PORT:-$((28000 + (RANDOM % 2000)))}"
SLOT_BIAS="${ONTOLITH_DRILL_SLOT_BIAS:-256}"
SECRET="drill-raft-secret-${RANDOM}"
ELECTION_WAIT="${ONTOLITH_DRILL_ELECTION_WAIT:-90}"
CONVERGE_WAIT="${ONTOLITH_DRILL_CONVERGE_WAIT:-90}"
REJOIN_WAIT="${ONTOLITH_DRILL_REJOIN_WAIT:-120}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ontolith-drill-XXXXXX")"
EVIDENCE_DIR="${ONTOLITH_DRILL_EVIDENCE_DIR:-$WORK/evidence}"
LOG_DIR="$WORK/logs"
mkdir -p "$EVIDENCE_DIR" "$LOG_DIR"
for i in $(seq 0 $((NODES - 1))); do mkdir -p "$WORK/d$i"; done
TRANSCRIPT="$EVIDENCE_DIR/drill-transcript.txt"

PIDS=()
step_num=0
log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$TRANSCRIPT"; }
fail() { log "FAIL: $*"; exit 1; }
ok() { log "OK: $*"; }

restart_node() {
  local i="$1"
  eval p=\$p$i; eval m=\$m$i
  ONTOLITH_CLUSTER_MODE=raft \
  ONTOLITH_RAFT_NODE_ID="$i" \
  ONTOLITH_RAFT_LISTEN="127.0.0.1:${p}" \
  ONTOLITH_RAFT_SECRET="$SECRET" \
  ONTOLITH_RAFT_MEMBERS="$MEMBERS" \
  ONTOLITH_RAFT_STORAGE_PATH="$WORK/d$i" \
  ONTOLITH_RAFT_SLOT_BIAS="$SLOT_BIAS" \
  ONTOLITH_MANAGEMENT_BIND="127.0.0.1:${m}" \
  "$BIN" >"$LOG_DIR/node$i.log" 2>&1 &
  PIDS[$i]=$!
}

wait_rejoin() {
  local i="$1" target="$2"
  eval m=\$m$i
  for _ in $(seq 1 "$REJOIN_WAIT"); do
    c="$(commit_of "$m")"
    if [[ -n "$c" && "$c" =~ ^[0-9]+$ && "$c" -ge "$target" ]]; then
      ok "node n${i} rejoined and caught up to commit ${target}"
      return 0
    fi
    sleep 1
  done
  log "restart log:"; tail -20 "$LOG_DIR/node$i.log"
  fail "node n${i} did not rejoin / catch up to commit ${target}"
}

cleanup() {
  for pid in "${PIDS[@]:-}"; do
    if [[ -n "$pid" ]]; then kill "$pid" >/dev/null 2>&1 || true; fi
  done
  wait "${PIDS[@]:-}" 2>/dev/null || true
}
trap cleanup EXIT

[[ -x "$BIN" ]] || fail "missing binary $BIN (run: cargo build -p ontolith-server --bin ontolith-management-server)"

mon() { curl -fsS --max-time 2 "http://127.0.0.1:$1/admin/monitoring" 2>/dev/null || true; }
field() { echo "$1" | grep -o "\"$2\":[^,}]*" | head -1 | sed 's/^[^:]*://' | tr -d '"' || true; }
commit_of() { field "$(mon "$1")" commit_index; }
leader_of() { field "$(mon "$1")" leader; }

MEMBERS=""
for i in $(seq 0 $((NODES - 1))); do
  eval p$i=$((BASE_PORT + i))
  eval m$i=$((BASE_PORT + 100 + i))
  if [[ -n "$MEMBERS" ]]; then MEMBERS="${MEMBERS},"; fi
  MEMBERS="${MEMBERS}n$i=http://127.0.0.1:$((BASE_PORT + i))"
done

log "=== Ontolith rebalance + DR drill (nodes=$NODES, ports=${BASE_PORT}+, slot_bias=${SLOT_BIAS}) ==="
log "workdir: $WORK"

for i in $(seq 0 $((NODES - 1))); do
  eval p=\$p$i; eval m=\$m$i
  ONTOLITH_CLUSTER_MODE=raft \
  ONTOLITH_RAFT_NODE_ID="$i" \
  ONTOLITH_RAFT_LISTEN="127.0.0.1:${p}" \
  ONTOLITH_RAFT_SECRET="$SECRET" \
  ONTOLITH_RAFT_MEMBERS="$MEMBERS" \
  ONTOLITH_RAFT_STORAGE_PATH="$WORK/d$i" \
  ONTOLITH_RAFT_SLOT_BIAS="$SLOT_BIAS" \
  ONTOLITH_MANAGEMENT_BIND="127.0.0.1:${m}" \
  "$BIN" >"$LOG_DIR/node$i.log" 2>&1 &
  PIDS+=($!)
done
log "booted $NODES nodes"

# 1. election
leader=""
for _ in $(seq 1 "$ELECTION_WAIT"); do
  for i in $(seq 0 $((NODES - 1))); do
    eval m=\$m$i
    l="$(leader_of "$m")"
    if [[ -n "$l" && "$l" =~ ^n[0-9]+$ ]]; then leader=$l; break 2; fi
  done
  sleep 1
done
[[ -n "$leader" ]] || { log "node logs:"; for i in $(seq 0 $((NODES - 1))); do log "--- n$i ---"; tail -5 "$LOG_DIR/node$i.log"; done; fail "no leader elected within ${ELECTION_WAIT}s"; }
lid="${leader#n}"
eval lm=\$m$lid
log "elected leader: $leader (admin http://127.0.0.1:${lm})"

# 2. online rebalance
shard_map_epoch_before="$(field "$(mon "$lm")" shard_map_epoch)"
rebalance_body="$(curl -fsS --max-time 10 -X POST "http://127.0.0.1:${lm}/admin/data/rebalance")"
plans="$(echo "$rebalance_body" | grep -o '"plans":[0-9]*' | head -1 | cut -d: -f2)"
shard_map_epoch_after="$(echo "$rebalance_body" | grep -o '"shard_map_epoch":[0-9]*' | head -1 | cut -d: -f2)"
log "rebalance: $rebalance_body"
(( plans > 0 )) || fail "rebalance produced 0 plans (slot_bias=${SLOT_BIAS}); online rebalance has nothing to move"
(( shard_map_epoch_after > shard_map_epoch_before )) || fail "rebalance did not advance shard-map epoch (${shard_map_epoch_before} -> ${shard_map_epoch_after})"
ok "online rebalance moved ${plans} slot ranges, shard-map epoch ${shard_map_epoch_before} -> ${shard_map_epoch_after}"

# 3. replication baseline
for _ in 1 2 3; do curl -fsS --max-time 10 -X POST "http://127.0.0.1:${lm}/admin/data/replicate?append=1" >/dev/null; done
commit="$(commit_of "$lm")"
[[ "$commit" =~ ^[0-9]+$ ]] || fail "no commit index after baseline replication"
converged=0
for _ in $(seq 1 "$CONVERGE_WAIT"); do
  converged=1
  for i in $(seq 0 $((NODES - 1))); do
    eval m=\$m$i
    c="$(commit_of "$m")"
    [[ -n "$c" && "$c" =~ ^[0-9]+$ && "$c" -ge "$commit" ]] || { converged=0; break; }
  done
  (( converged == 1 )) && break
  sleep 1
done
(( converged == 1 )) || fail "nodes did not converge to commit ${commit}"
ok "baseline replication: all ${NODES} nodes at commit ${commit}"

# 4. DR: follower loss
follower=""
for i in $(seq 0 $((NODES - 1))); do
  if [[ "$i" != "$lid" ]]; then follower=$i; break; fi
done
log "DR-1: killing follower n${follower}"
kill "${PIDS[$follower]}" >/dev/null 2>&1 || true
wait "${PIDS[$follower]}" 2>/dev/null || true
PIDS[$follower]=""
curl -fsS --max-time 10 -X POST "http://127.0.0.1:${lm}/admin/data/replicate?append=1" >/dev/null
commit2="$(commit_of "$lm")"
(( commit2 > commit )) || fail "commit did not advance with one follower down (${commit} -> ${commit2})"
ok "majority commit survived follower loss (${commit} -> ${commit2})"

# 5. restart the lost follower (back to a full 3-node membership)
log "DR-1b: restarting follower n${follower}"
restart_node "$follower"
wait_rejoin "$follower" "$commit2"

# 6. DR: leader loss
log "DR-2: killing leader n${lid}"
kill "${PIDS[$lid]}" >/dev/null 2>&1 || true
wait "${PIDS[$lid]}" 2>/dev/null || true
PIDS[$lid]=""
newleader=""
for _ in $(seq 1 "$ELECTION_WAIT"); do
  for i in $(seq 0 $((NODES - 1))); do
    [[ "$i" == "$lid" ]] && continue
    eval m=\$m$i
    l="$(leader_of "$m")"
    if [[ -n "$l" && "$l" =~ ^n[0-9]+$ && "$l" != "n$lid" ]]; then newleader=$l; break 2; fi
  done
  sleep 1
done
[[ -n "$newleader" ]] || fail "no new leader elected after leader n${lid} loss"
newid="${newleader#n}"
eval nlm=\$m$newid
log "new leader after failover: ${newleader}"
curl -fsS --max-time 10 -X POST "http://127.0.0.1:${nlm}/admin/data/replicate?append=1" >/dev/null
commit3="$(commit_of "$nlm")"
(( commit3 > commit2 )) || fail "commit did not advance after failover (${commit2} -> ${commit3})"
ok "automatic failover: commit advanced ${commit2} -> ${commit3} under ${newleader}"

# 7. DR: restart / rejoin the lost leader
log "DR-3: restarting n${lid} on original storage"
restart_node "$lid"
wait_rejoin "$lid" "$commit3"

log "=== DRILL PASS ==="
log "evidence: $TRANSCRIPT"
