#!/usr/bin/env bash
# Evaluate persisted runtime_probe samples over a day/week window (SLO history).
#
# Alert policy (exit 1 on any breach):
#   - success_percent < ONTOLITH_SLO_MIN_SUCCESS_PERCENT (default 99)
#   - window p95 latency > ONTOLITH_SLO_P95_MAX_LATENCY_MS (default 250)
#   - trailing consecutive unreachable samples >= ONTOLITH_SLO_MAX_CONSECUTIVE_FAILURES (default 3)
#   - window p95 latency > previous-window p95 * ONTOLITH_SLO_LATENCY_SPIKE_FACTOR (default 2.0)
#
# Usage:
#   bash scripts/check-slo-window-history.sh [--window-hours N] [--self-test]
set -euo pipefail

state_dir="${ONTOLITH_SLO_STATE_DIR:-${HOME}/.local/state/ontolith/slo}"
samples_file="${state_dir}/samples.jsonl"
reports_file="${state_dir}/reports.jsonl"

window_hours="${ONTOLITH_SLO_WINDOW_HOURS:-24}"
min_success_percent="${ONTOLITH_SLO_MIN_SUCCESS_PERCENT:-99}"
p95_max_latency_ms="${ONTOLITH_SLO_P95_MAX_LATENCY_MS:-250}"
max_consecutive_failures="${ONTOLITH_SLO_MAX_CONSECUTIVE_FAILURES:-3}"
latency_spike_factor="${ONTOLITH_SLO_LATENCY_SPIKE_FACTOR:-2.0}"

if [[ "${1:-}" == "--window-hours" ]]; then
  window_hours="${2:?missing window hours}"
  shift 2
fi

json_field() {
  local line="$1" field="$2"
  echo "${line}" | sed -n "s/.*\"${field}\":\([^,}]*\).*/\1/p"
}

p95() {
  local values count rank value
  values="$(cat)"
  count="$(echo "${values}" | wc -l | tr -d ' ')"
  if [[ "${count}" == "0" ]]; then
    return 1
  fi
  rank=$(((95 * count + 99) / 100))
  value="$(echo "${values}" | sort -n | sed -n "${rank}p")"
  echo "${value}"
}

# Synthesize a history file in a temp state dir and verify pass/breach logic.
self_test() {
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' RETURN
  local now
  now="$(date +%s)"
  local i

  # --- case 1: healthy window (success 100%, low p95) ---
  : > "${tmpdir}/samples.jsonl"
  for i in $(seq 1 20); do
    echo "{\"ts\":$((now - i * 3600)),\"reachable\":true,\"latency_ms\":$((40 + i * 5))}" >> "${tmpdir}/samples.jsonl"
  done
  ONTOLITH_SLO_STATE_DIR="${tmpdir}" bash "$0" --window-hours 48
  echo "self-test: healthy window passed"

  # --- case 2: consecutive failures breach ---
  : > "${tmpdir}/samples.jsonl"
  for i in $(seq 1 10); do
    echo "{\"ts\":$((now - i * 3600)),\"reachable\":true,\"latency_ms\":80}" >> "${tmpdir}/samples.jsonl"
  done
  for i in 1 2 3 4; do
    echo "{\"ts\":$((now - 600 + i)),\"reachable\":false,\"latency_ms\":null}" >> "${tmpdir}/samples.jsonl"
  done
  if ONTOLITH_SLO_STATE_DIR="${tmpdir}" ONTOLITH_SLO_MAX_CONSECUTIVE_FAILURES=3 bash "$0" --window-hours 24 >/dev/null 2>&1; then
    echo "self-test FAILED: consecutive failures did not breach" >&2
    return 1
  fi
  echo "self-test: consecutive-failures breach passed"

  # --- case 3: p95 latency breach ---
  : > "${tmpdir}/samples.jsonl"
  for i in $(seq 1 10); do
    echo "{\"ts\":$((now - i * 3600)),\"reachable\":true,\"latency_ms\":$((500 + i))}" >> "${tmpdir}/samples.jsonl"
  done
  if ONTOLITH_SLO_STATE_DIR="${tmpdir}" ONTOLITH_SLO_P95_MAX_LATENCY_MS=250 bash "$0" --window-hours 24 >/dev/null 2>&1; then
    echo "self-test FAILED: p95 latency did not breach" >&2
    return 1
  fi
  echo "self-test: p95-latency breach passed"

  # --- case 4: latency spike vs previous window ---
  : > "${tmpdir}/samples.jsonl"
  for i in $(seq 1 10); do
    echo "{\"ts\":$((now - 12 * 3600 - i * 3600)),\"reachable\":true,\"latency_ms\":100}" >> "${tmpdir}/samples.jsonl"
  done
  for i in $(seq 1 10); do
    echo "{\"ts\":$((now - i * 600)),\"reachable\":true,\"latency_ms\":$((900 + i))}" >> "${tmpdir}/samples.jsonl"
  done
  if ONTOLITH_SLO_STATE_DIR="${tmpdir}" ONTOLITH_SLO_LATENCY_SPIKE_FACTOR=2.0 bash "$0" --window-hours 6 >/dev/null 2>&1; then
    echo "self-test FAILED: latency spike did not breach" >&2
    return 1
  fi
  echo "self-test: latency-spike breach passed"
  return 0
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit $?
fi

for numeric_value in "${window_hours}" "${min_success_percent}" "${p95_max_latency_ms}" "${max_consecutive_failures}"; do
  if [[ ! "${numeric_value}" =~ ^[0-9]+$ ]]; then
    echo "invalid numeric input: ${numeric_value}" >&2
    exit 2
  fi
done
if ! awk -v f="${latency_spike_factor}" 'BEGIN{exit !(f+0>0)}'; then
  echo "invalid latency_spike_factor: ${latency_spike_factor}" >&2
  exit 2
fi

mkdir -p "${state_dir}"
if [[ ! -f "${samples_file}" ]]; then
  echo "SLO history empty: ${samples_file} (run ontolith-slo-collect first)" >&2
  exit 2
fi

now="$(date +%s)"
window_sec=$((window_hours * 3600))
tmp_lat="$("mktemp")"
tmp_prev="$("mktemp")"
trap 'rm -f "${tmp_lat}" "${tmp_prev}"' EXIT

success_count=0
failure_count=0
consecutive_failures=0
max_consecutive=0

while IFS= read -r line; do
  ts="$(json_field "${line}" ts)"
  reachable="$(json_field "${line}" reachable)"
  latency="$(json_field "${line}" latency_ms)"
  [[ "${ts}" =~ ^[0-9]+$ ]] || continue

  if (( ts >= now - window_sec )); then
    if [[ "${reachable}" == "true" && "${latency}" =~ ^[0-9]+$ ]]; then
      success_count=$((success_count + 1))
      echo "${latency}" >> "${tmp_lat}"
      consecutive_failures=0
    else
      failure_count=$((failure_count + 1))
      consecutive_failures=$((consecutive_failures + 1))
      if (( consecutive_failures > max_consecutive )); then
        max_consecutive="${consecutive_failures}"
      fi
    fi
  elif (( ts >= now - 2 * window_sec )); then
    if [[ "${reachable}" == "true" && "${latency}" =~ ^[0-9]+$ ]]; then
      echo "${latency}" >> "${tmp_prev}"
    fi
  fi
done < "${samples_file}"

total=$((success_count + failure_count))
if (( total == 0 )); then
  echo "SLO window breach: no samples in the last ${window_hours}h" >&2
  echo "{\"ts\":${now},\"window_hours\":${window_hours},\"samples\":0,\"success_percent\":0,\"p95_latency_ms\":null,\"prev_p95_latency_ms\":null,\"consecutive_failures\":0,\"passed\":false,\"reason\":\"no_samples\"}" >> "${reports_file}"
  exit 1
fi

success_percent=$((success_count * 100 / total))

window_p95=""
prev_p95=""
if [[ -s "${tmp_lat}" ]]; then
  window_p95="$(p95 < "${tmp_lat}")"
fi
if [[ -s "${tmp_prev}" ]]; then
  prev_p95="$(p95 < "${tmp_prev}")"
fi

window_p95_int="${window_p95:-0}"
prev_p95_int="${prev_p95:-0}"
spike_breach=0
if (( prev_p95_int > 0 && window_p95_int > 0 )); then
  spike_limit="$(awk -v a="${prev_p95_int}" -v f="${latency_spike_factor}" 'BEGIN{printf "%d", a * f}')"
  if (( window_p95_int > spike_limit )); then
    spike_breach=1
  fi
fi

passed=1
reason=""
if (( success_percent < min_success_percent )); then
  passed=0
  reason="success_percent_${success_percent}_below_${min_success_percent}"
elif (( max_consecutive >= max_consecutive_failures )); then
  passed=0
  reason="consecutive_failures_${max_consecutive}_gte_${max_consecutive_failures}"
elif (( window_p95_int > 0 && window_p95_int > p95_max_latency_ms )); then
  passed=0
  reason="p95_${window_p95_int}_above_${p95_max_latency_ms}"
elif (( spike_breach == 1 )); then
  passed=0
  reason="latency_spike_p95_${window_p95_int}_vs_prev_${prev_p95_int}"
fi

echo "SLO window summary:"
echo "  window_hours=${window_hours} samples=${total} success=${success_count} failure=${failure_count} success_percent=${success_percent}%"
echo "  p95_latency_ms=${window_p95:-n/a} prev_window_p95_ms=${prev_p95:-n/a}"
echo "  consecutive_failures(max)=${max_consecutive} thresholds: success>=${min_success_percent}% p95<=${p95_max_latency_ms}ms failures<${max_consecutive_failures} spike<=${latency_spike_factor}x"

echo "{\"ts\":${now},\"window_hours\":${window_hours},\"samples\":${total},\"success_percent\":${success_percent},\"p95_latency_ms\":${window_p95:-null},\"prev_p95_latency_ms\":${prev_p95:-null},\"consecutive_failures\":${max_consecutive},\"passed\":${passed},\"reason\":\"${reason}\"}" >> "${reports_file}"

if (( passed == 1 )); then
  echo "SLO window check passed"
  exit 0
fi

# Alert policy: persist breach to alerts.jsonl for external hooks (webhook/mail).
alerts_file="${state_dir}/alerts.jsonl"
echo "{\"ts\":${now},\"window_hours\":${window_hours},\"success_percent\":${success_percent},\"p95_latency_ms\":${window_p95:-null},\"consecutive_failures\":${max_consecutive},\"reason\":\"${reason}\"}" >> "${alerts_file}"
echo "SLO window check FAILED: ${reason}" >&2
echo "alert persisted: ${alerts_file}"
exit 1
