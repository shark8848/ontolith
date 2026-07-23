#!/usr/bin/env bash
# Evaluate management runtime_probe SLOs over a short observation window.
set -euo pipefail

monitoring_url="${ONTOLITH_MANAGEMENT_MONITORING_URL:-http://127.0.0.1:9091/admin/monitoring}"
window_samples="${ONTOLITH_MANAGEMENT_SLO_WINDOW_SAMPLES:-12}"
window_interval_sec="${ONTOLITH_MANAGEMENT_SLO_WINDOW_INTERVAL_SEC:-5}"
min_success_percent="${ONTOLITH_MANAGEMENT_SLO_MIN_SUCCESS_PERCENT:-99}"
p95_max_latency_ms="${ONTOLITH_MANAGEMENT_SLO_P95_MAX_LATENCY_MS:-250}"

for numeric_value in "${window_samples}" "${window_interval_sec}" "${min_success_percent}" "${p95_max_latency_ms}"; do
  if [[ ! "${numeric_value}" =~ ^[0-9]+$ ]]; then
    echo "invalid numeric input: ${numeric_value}" >&2
    exit 2
  fi
done

if (( window_samples == 0 )); then
  echo "ONTOLITH_MANAGEMENT_SLO_WINDOW_SAMPLES must be > 0" >&2
  exit 2
fi

latency_file="$(mktemp)"
trap 'rm -f "${latency_file}"' EXIT

success_count=0
failure_count=0

for ((sample_idx = 1; sample_idx <= window_samples; sample_idx++)); do
  payload=""
  if payload="$(curl -fsS "${monitoring_url}" 2>/dev/null)"; then
    reachable="$(echo "${payload}" | sed -n 's/.*"runtime_probe":{[^}]*"reachable":\([a-z]*\).*/\1/p')"
    latency_ms="$(echo "${payload}" | sed -n 's/.*"runtime_probe":{[^}]*"latency_ms":\([0-9][0-9]*\).*/\1/p')"

    if [[ "${reachable}" == "true" && -n "${latency_ms}" ]]; then
      success_count=$((success_count + 1))
      echo "${latency_ms}" >> "${latency_file}"
    else
      failure_count=$((failure_count + 1))
    fi
  else
    failure_count=$((failure_count + 1))
  fi

  if (( sample_idx < window_samples && window_interval_sec > 0 )); then
    sleep "${window_interval_sec}"
  fi
done

success_percent=$((success_count * 100 / window_samples))
latency_count="$(wc -l < "${latency_file}" | tr -d ' ')"

if (( latency_count == 0 )); then
  echo "management SLO window check failed: no successful runtime_probe samples"
  echo "window_samples=${window_samples} success_count=${success_count} failure_count=${failure_count}"
  exit 1
fi

p95_rank=$(((95 * latency_count + 99) / 100))
p95_latency_ms="$(sort -n "${latency_file}" | sed -n "${p95_rank}p")"
max_latency_ms="$(sort -nr "${latency_file}" | head -n 1)"

echo "management SLO window summary:"
echo "  monitoring_url=${monitoring_url}"
echo "  samples=${window_samples} success=${success_count} failure=${failure_count} success_percent=${success_percent}%"
echo "  latency_count=${latency_count} p95_latency_ms=${p95_latency_ms} max_latency_ms=${max_latency_ms}"
echo "  thresholds: min_success_percent=${min_success_percent}% p95_max_latency_ms=${p95_max_latency_ms}"

if (( success_percent < min_success_percent )); then
  echo "management SLO window check failed: success_percent ${success_percent}% < ${min_success_percent}%"
  exit 1
fi

if (( p95_latency_ms > p95_max_latency_ms )); then
  echo "management SLO window check failed: p95 latency ${p95_latency_ms}ms > ${p95_max_latency_ms}ms"
  exit 1
fi

echo "management SLO window check passed"
