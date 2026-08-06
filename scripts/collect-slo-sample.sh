#!/usr/bin/env bash
# Collect one management runtime_probe sample into the SLO history file.
# Feed by systemd timer (ontolith-slo-collect.timer) for day/week window SLOs.
set -euo pipefail

monitoring_url="${ONTOLITH_MANAGEMENT_MONITORING_URL:-http://127.0.0.1:9091/admin/monitoring}"
state_dir="${ONTOLITH_SLO_STATE_DIR:-${HOME}/.local/state/ontolith/slo}"

mkdir -p "${state_dir}"
samples_file="${state_dir}/samples.jsonl"
ts="$(date +%s)"

payload="$(curl -fsS "${monitoring_url}" 2>/dev/null || true)"
if [[ -z "${payload}" ]]; then
  echo "{\"ts\":${ts},\"reachable\":false,\"latency_ms\":null}" >> "${samples_file}"
  echo "sample: unreachable"
  exit 0
fi

reachable="$(echo "${payload}" | sed -n 's/.*"runtime_probe":{[^}]*"reachable":\([a-z]*\).*/\1/p')"
latency_ms="$(echo "${payload}" | sed -n 's/.*"runtime_probe":{[^}]*"latency_ms":\([0-9][0-9]*\).*/\1/p')"

if [[ "${reachable}" == "true" && -n "${latency_ms}" ]]; then
  echo "{\"ts\":${ts},\"reachable\":true,\"latency_ms\":${latency_ms}}" >> "${samples_file}"
  echo "sample: ok latency_ms=${latency_ms}"
else
  echo "{\"ts\":${ts},\"reachable\":false,\"latency_ms\":null}" >> "${samples_file}"
  echo "sample: not-reachable-or-missing-latency"
fi
