#!/usr/bin/env bash
# Install ontolith-server as a *system* systemd unit (requires sudo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="${ROOT}/target/release/ontolith-server"
UNIT_SRC="${ROOT}/deployments/ontolith-server.service"
ENV_SRC="${ROOT}/deployments/ontolith.env"

if [[ ! -x "${BIN_SRC}" ]]; then
  echo "error: release binary missing: ${BIN_SRC}"
  echo "run: cargo build -p ontolith-server --release"
  exit 1
fi

echo "==> install binary /usr/local/bin/ontolith-server"
sudo install -m 755 "${BIN_SRC}" /usr/local/bin/ontolith-server

echo "==> config + data dirs"
sudo install -d -m 755 /etc/ontolith
sudo install -d -o "${USER}" -g "${USER}" -m 755 /var/lib/ontolith /var/lib/ontolith/data
sudo install -d -o "${USER}" -g "${USER}" -m 755 /home/ontolith/data

if [[ ! -f /etc/ontolith/ontolith.env ]]; then
  sudo install -m 644 "${ENV_SRC}" /etc/ontolith/ontolith.env
  echo "installed /etc/ontolith/ontolith.env"
else
  echo "kept existing /etc/ontolith/ontolith.env"
fi

echo "==> systemd unit"
sudo install -m 644 "${UNIT_SRC}" /etc/systemd/system/ontolith-server.service
sudo systemctl daemon-reload
sudo systemctl enable ontolith-server.service
sudo systemctl restart ontolith-server.service
sleep 1
sudo systemctl --no-pager --full status ontolith-server.service || true

echo
curl -fsS http://127.0.0.1:8080/health && echo || {
  echo "health failed; see: sudo journalctl -u ontolith-server -n 50 --no-pager"
}

echo
echo "Commands: sudo systemctl {status|restart|stop} ontolith-server"
echo "Logs:     sudo journalctl -u ontolith-server -f"
