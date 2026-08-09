#!/usr/bin/env bash
# P8-02: semantic retrieval bench threshold assertions + trend recording.
#
# Runs `cargo bench -p ontolith-ai --bench semantic_bench` and asserts each
# case stays under a per-case ns/op ceiling (regression guard). Every run
# appends one JSONL line per case to the trend file, so sustained regressions
# are visible over time.
#
# Thresholds (ns/op) are overridable per case:
#   ONTOLITH_BENCH_SEARCH_MAX_NS       (default 2000000)   search top-10 (10k)
#   ONTOLITH_BENCH_UPSERT_MAX_NS       (default 100000)    single-term upsert
#   ONTOLITH_BENCH_EMBED_ONLY_MAX_NS   (default 1000000)   embed-only top-10 (KPI <1ms)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

search_max="${ONTOLITH_BENCH_SEARCH_MAX_NS:-2000000}"
upsert_max="${ONTOLITH_BENCH_UPSERT_MAX_NS:-100000}"
embed_only_max="${ONTOLITH_BENCH_EMBED_ONLY_MAX_NS:-1000000}"
trend_path="${ONTOLITH_BENCH_TREND_PATH:-$ROOT/benchmarks/trends/semantic-bench.jsonl}"
run_id="${ONTOLITH_BENCH_RUN_ID:-$(git rev-parse --short HEAD 2>/dev/null || echo local)}"
mkdir -p "$(dirname "$trend_path")"

out="$(mktemp)"
trap 'rm -f "$out"' EXIT
ONTOLITH_BENCH_TREND_PATH="$trend_path" ONTOLITH_BENCH_RUN_ID="$run_id" \
  cargo bench -p ontolith-ai --bench semantic_bench 2>&1 | tee "$out"

fail=0
while IFS= read -r line; do
  case "$line" in
    *"ns/op"*)
      name="$(echo "$line" | sed 's/[[:space:]]*$//')"
      ns="$(echo "$line" | awk '/ns\/op/{for (i = 1; i <= NF; i++) if ($i == "ns/op") print $(i - 1)}')"
      max=""
      case "$name" in
        "semantic search top-10"*) max="$search_max" ;;
        "semantic index upsert"*) max="$upsert_max" ;;
        "semantic search embed-only"*) max="$embed_only_max" ;;
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
  echo "semantic bench threshold breach (P8-02 KPI: embed-only top-10 < 1ms)"
  exit 1
fi
echo "semantic bench thresholds OK; trend appended to ${trend_path}"
