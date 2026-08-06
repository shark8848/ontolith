#!/usr/bin/env bash
# Install Ontolith SLO history automation as user systemd timers (no root).
# Enables: sample collector (5min), daily window check, weekly window check.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT_SRC="${ROOT}/deployments/systemd-user"
ENV_SRC="${ROOT}/deployments/ontolith-slo.env"
UNIT_DST="${HOME}/.config/systemd/user"
ENV_DST="${HOME}/.config/ontolith/ontolith-slo.env"

for unit in ontolith-slo-collect ontolith-slo-daily ontolith-slo-weekly; do
  if [[ ! -f "${UNIT_SRC}/${unit}.service" || ! -f "${UNIT_SRC}/${unit}.timer" ]]; then
    echo "error: missing unit files for ${unit} in ${UNIT_SRC}" >&2
    exit 1
  fi
done
if [[ ! -x "${ROOT}/scripts/collect-slo-sample.sh" || ! -x "${ROOT}/scripts/check-slo-window-history.sh" ]]; then
  echo "error: SLO scripts not executable in ${ROOT}/scripts" >&2
  exit 1
fi

mkdir -p "${UNIT_DST}" "${HOME}/.config/ontolith"

for unit in ontolith-slo-collect ontolith-slo-daily ontolith-slo-weekly; do
  echo "==> install ${unit}"
  install -m 644 "${UNIT_SRC}/${unit}.service" "${UNIT_DST}/"
  install -m 644 "${UNIT_SRC}/${unit}.timer" "${UNIT_DST}/"
done

if [[ ! -f "${ENV_DST}" ]]; then
  echo "==> install env: ${ENV_DST}"
  install -m 644 "${ENV_SRC}" "${ENV_DST}"
else
  echo "==> keep existing ${ENV_DST}"
fi

echo "==> reload user systemd"
systemctl --user daemon-reload
for unit in ontolith-slo-collect ontolith-slo-daily ontolith-slo-weekly; do
  systemctl --user enable "${unit}.timer"
  systemctl --user start "${unit}.timer"
done

echo
echo "==> timers:"
systemctl --user list-timers 'ontolith-slo-*' --no-pager || true
echo
echo "Commands:"
echo "  systemctl --user list-timers 'ontolith-slo-*'"
echo "  systemctl --user start ontolith-slo-daily.service   # run daily check now"
echo "  systemctl --user start ontolith-slo-weekly.service  # run weekly check now"
echo "  journalctl --user -u ontolith-slo-collect -u ontolith-slo-daily -n 30"
echo
echo "SLO history state: \${ONTOLITH_SLO_STATE_DIR:-~/.local/state/ontolith/slo}"
echo "  samples.jsonl / reports.jsonl / alerts.jsonl"
