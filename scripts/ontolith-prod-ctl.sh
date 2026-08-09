#!/usr/bin/env bash
# Ontolith production daemon control (container env: no systemd/sudo).
# Usage: ontolith-prod-ctl.sh {start|stop|status|restart|logs}
# Root: /home/ontolith/prod — bin/, data/, logs/, run/, ontolith.env, ontolith-management.env
set -euo pipefail

PROD="${ONTOLITH_PROD_ROOT:-/home/ontolith/prod}"
GW_BIN="$PROD/bin/ontolith-server"
MG_BIN="$PROD/bin/ontolith-management-server"
GW_ENV="$PROD/ontolith.env"
MG_ENV="$PROD/ontolith-management.env"
RUN="$PROD/run"
LOG="$PROD/logs"

is_up() { # pidfile
  [[ -f "$1" ]] && kill -0 "$(cat "$1")" 2>/dev/null
}

start_one() { # name bin envfile pidfile logfile
  local name="$1" bin="$2" envf="$3" pidf="$4" logf="$5"
  if is_up "$pidf"; then echo "==> $name already running (pid $(cat "$pidf"))"; return 0; fi
  echo "==> starting $name"
  # shellcheck disable=SC2046
  if command -v setsid >/dev/null 2>&1; then
    setsid env $(grep -vE '^#|^$' "$envf" | xargs) "$bin" >>"$logf" 2>&1 </dev/null &
  else
    nohup env $(grep -vE '^#|^$' "$envf" | xargs) "$bin" >>"$logf" 2>&1 </dev/null &
  fi
  disown 2>/dev/null || true
  echo $! > "$pidf"
  local i
  for i in $(seq 1 30); do
    is_up "$pidf" || { echo "!! $name exited early"; tail -5 "$logf"; return 1; }
    sleep 0.5
  done
}

start() {
  mkdir -p "$RUN"
  start_one gateway "$GW_BIN" "$GW_ENV" "$RUN/gateway.pid" "$LOG/gateway.log"
  start_one management "$MG_BIN" "$MG_ENV" "$RUN/management.pid" "$LOG/management.log"
  sleep 1
  status
}

stop() {
  for name in gateway management; do
    pidf="$RUN/$name.pid"
    if is_up "$pidf"; then
      pid="$(cat "$pidf")"; echo "==> stopping $name (pid $pid)"
      kill "$pid" 2>/dev/null || true
      for _ in $(seq 1 20); do is_up "$pidf" || break; sleep 0.5; done
      is_up "$pidf" && { echo "!! $name did not stop; force kill"; kill -9 "$pid" 2>/dev/null || true; }
    fi
    rm -f "$pidf"
  done
  echo "==> stopped"
}

status() {
  for name in gateway management; do
    pidf="$RUN/$name.pid"
    if is_up "$pidf"; then echo "==> $name: RUNNING pid $(cat "$pidf")"; else echo "==> $name: stopped"; fi
  done
}

logs() { tail -n 40 "$LOG/gateway.log" "$LOG/management.log"; }

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  restart) stop; sleep 1; start ;;
  status) status ;;
  logs) logs ;;
  *) echo "usage: $0 {start|stop|status|restart|logs}"; exit 2 ;;
esac
