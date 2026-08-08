#!/usr/bin/env bash
# P7-02: storage bench threshold assertions + trend recording.
#
# Runs `cargo bench -p ontolith-storage` and asserts each case stays under a
# per-case ns/op ceiling (regression guard). Every run appends one JSONL line
# per case to the trend file, so sustained regressions are visible over time.
#
# Thresholds (ns/op) are overridable per case:
#   ONTOLITH_BENCH_DICT_MAX_NS    (default 5000)    dict encode_node
#   ONTOLITH_BENCH_INSERT_MAX_NS  (default 20000)   triple insert + commit
#   ONTOLITH_BENCH_MATCH_MAX_NS   (default 5000000) match by subject (1k)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

dict_max="${ONTOLITH_BENCH_DICT_MAX_NS:-5000}"
insert_max="${ONTOLITH_BENCH_INSERT_MAX_NS:-20000}"
match_max="${ONTOLITH_BENCH_MATCH_MAX_NS:-5000000}"
trend_path="${ONTOLITH_BENCH_TREND_PATH:-$ROOT/benchmarks/trends/storage-bench.jsonl}"
run_id="${ONTOLITH_BENCH_RUN_ID:-$(git rev-parse --short HEAD 2>/dev/null || echo local)}"
mkdir -p "$(dirname "$trend_path")"

out="$(mktemp)"
trap 'rm -f "$out"' EXIT
ONTOLITH_BENCH_TREND_PATH="$trend_path" ONTOLITH_BENCH_RUN_ID="$run_id" \
  cargo bench -p ontolith-storage --bench storage_bench 2>&1 | tee "$out"

fail=0
while IFS= read -r line; do
  case "$line" in
    *"ns/op"*)
      name="$(echo "$line" | sed 's/[[:space:]]*$//')"
      ns="$(echo "$line" | awk '/ns\/op/{for (i = 1; i <= NF; i++) if ($i == "ns/op") print $(i - 1)}')"
      max=""
      case "$name" in
        "dict encode_node"*) max="$dict_max" ;;
        "triple insert + commit"*) max="$insert_max" ;;
        "match by subject"*) max="$match_max" ;;
      esac
      if [[ -n "$max" ]]; then
        if [[ "$ns" =~ ^[0-9]+$ && "$ns" -le "$max" ]]; then
          echo "PASS ${name}: ${ns} ns/op <= ${max} ns/op"
        else
          echo "FAIL ${name}: ${ns} ns/op > ${max} ns/op"
          fail=1
        fi
      fi
      ;;
  esac
done < "$out"

if (( fail == 1 )); then
  echo "bench threshold breach (see benchmarks/README.md for baselines)"
  exit 1
fi
echo "bench thresholds OK; trend appended to ${trend_path}"
