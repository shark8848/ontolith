#!/usr/bin/env bash
# Install ontolith-management-server as a system systemd unit (requires sudo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="${ROOT}/target/release/ontolith-management-server"
UNIT_SRC="${ROOT}/deployments/ontolith-management-server.service"
ENV_SRC="${ROOT}/deployments/ontolith-management.env"

if [[ ! -x "${BIN_SRC}" ]]; then
  echo "error: release binary missing: ${BIN_SRC}"
  echo "run: cargo build -p ontolith-server --release --bin ontolith-management-server"
  exit 1
fi

echo "==> install binary /usr/local/bin/ontolith-management-server"
sudo install -m 755 "${BIN_SRC}" /usr/local/bin/ontolith-management-server

echo "==> config + data dirs"
sudo install -d -m 755 /etc/ontolith
sudo install -d -o "${USER}" -g "${USER}" -m 755 /var/lib/ontolith /var/lib/ontolith/data
sudo install -d -o "${USER}" -g "${USER}" -m 755 /home/ontolith/data

if [[ ! -f /etc/ontolith/ontolith-management.env ]]; then
  sudo install -m 644 "${ENV_SRC}" /etc/ontolith/ontolith-management.env
  echo "installed /etc/ontolith/ontolith-management.env"
else
  echo "kept existing /etc/ontolith/ontolith-management.env"
fi

echo "==> systemd unit"
sudo install -m 644 "${UNIT_SRC}" /etc/systemd/system/ontolith-management-server.service
sudo systemctl daemon-reload
sudo systemctl enable ontolith-management-server.service
sudo systemctl restart ontolith-management-server.service
sleep 1
sudo systemctl --no-pager --full status ontolith-management-server.service || true

echo
BIND="$(grep -E '^ONTOLITH_MANAGEMENT_BIND=' /etc/ontolith/ontolith-management.env | cut -d= -f2- || echo '127.0.0.1:9091')"
echo "==> management health check ${BIND}"
if curl -fsS "http://${BIND}/admin/health"; then
  echo
else
  echo "health failed; see: sudo journalctl -u ontolith-management-server -n 50 --no-pager"
fi

echo
echo "Commands: sudo systemctl {status|restart|stop} ontolith-management-server"
echo "Logs:     sudo journalctl -u ontolith-management-server -f"
