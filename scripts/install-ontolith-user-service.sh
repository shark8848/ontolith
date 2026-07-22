#!/usr/bin/env bash
# Install ontolith-server as a *user* systemd service (no root required).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="${ROOT}/target/release/ontolith-server"
UNIT_SRC="${ROOT}/deployments/systemd-user/ontolith-server.service"
ENV_SRC="${ROOT}/deployments/ontolith.user.env"
UNIT_DST="${HOME}/.config/systemd/user/ontolith-server.service"
ENV_DST="${HOME}/.config/ontolith/ontolith.env"

if [[ ! -x "${BIN_SRC}" ]]; then
  echo "error: release binary missing: ${BIN_SRC}"
  echo "run: cargo build -p ontolith-server --release"
  exit 1
fi

mkdir -p "${HOME}/.config/systemd/user" "${HOME}/.config/ontolith" "${ROOT}/data"

echo "==> install user unit: ${UNIT_DST}"
install -m 644 "${UNIT_SRC}" "${UNIT_DST}"

if [[ ! -f "${ENV_DST}" ]]; then
  echo "==> install env: ${ENV_DST}"
  install -m 644 "${ENV_SRC}" "${ENV_DST}"
else
  echo "==> keep existing ${ENV_DST}"
fi

echo "==> reload user systemd"
systemctl --user daemon-reload
systemctl --user enable ontolith-server.service
systemctl --user restart ontolith-server.service

sleep 1
systemctl --user --no-pager --full status ontolith-server.service || true

BIND="$(grep -E '^ONTOLITH_BIND=' "${ENV_DST}" | cut -d= -f2- || echo '127.0.0.1:8081')"
echo
echo "==> health check ${BIND}"
if curl -fsS "http://${BIND}/health"; then
  echo
else
  echo "health failed; logs:"
  journalctl --user -u ontolith-server -n 30 --no-pager || true
fi

echo
echo "Commands:"
echo "  systemctl --user status ontolith-server"
echo "  systemctl --user restart ontolith-server"
echo "  systemctl --user stop ontolith-server"
echo "  journalctl --user -u ontolith-server -f"
echo
echo "Optional (survive logout; needs root once):"
echo "  sudo loginctl enable-linger $(whoami)"
