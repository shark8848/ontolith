#!/usr/bin/env bash
# Install ontolith-server as a systemd unit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="${ROOT}/target/release/ontolith-server"
UNIT_SRC="${ROOT}/deployments/ontolith-server.service"
ENV_SRC="${ROOT}/deployments/ontolith.env"

if [[ ! -x "${BIN_SRC}" ]]; then
  echo "error: release binary not found at ${BIN_SRC}"
  echo "run: cargo build -p ontolith-server --release"
  exit 1
fi

echo "==> install binary to /usr/local/bin/ontolith-server"
sudo install -m 755 "${BIN_SRC}" /usr/local/bin/ontolith-server

echo "==> create config/data directories"
sudo install -d -m 755 /etc/ontolith
sudo install -d -o sharkyai -g sharkyai -m 755 /var/lib/ontolith
sudo install -d -o sharkyai -g sharkyai -m 755 /var/lib/ontolith/data
sudo install -d -o sharkyai -g sharkyai -m 755 /home/ontolith/data

if [[ ! -f /etc/ontolith/ontolith.env ]]; then
  echo "==> install default env /etc/ontolith/ontolith.env"
  sudo install -m 644 "${ENV_SRC}" /etc/ontolith/ontolith.env
else
  echo "==> keep existing /etc/ontolith/ontolith.env"
fi

echo "==> install systemd unit"
sudo install -m 644 "${UNIT_SRC}" /etc/systemd/system/ontolith-server.service

echo "==> reload + enable + restart"
sudo systemctl daemon-reload
sudo systemctl enable ontolith-server.service
sudo systemctl restart ontolith-server.service

sleep 1
sudo systemctl --no-pager --full status ontolith-server.service || true

echo
echo "==> quick health check"
if curl -fsS "http://127.0.0.1:8080/health" 2>/dev/null; then
  echo
else
  # fallback: read bind from env
  BIND="$(grep -E '^ONTOLITH_BIND=' /etc/ontolith/ontolith.env | cut -d= -f2- || true)"
  echo "health probe failed on :8080; configured bind=${BIND:-unknown}"
  echo "check: journalctl -u ontolith-server -n 50 --no-pager"
fi

echo
echo "Useful commands:"
echo "  sudo systemctl status ontolith-server"
echo "  sudo systemctl restart ontolith-server"
echo "  sudo journalctl -u ontolith-server -f"
echo "  sudo systemctl stop ontolith-server"
echo "  sudo systemctl disable ontolith-server"
