#!/usr/bin/env bash
# P0-03 gate: every direct (non-path) dependency of the workspace must be
# registered in docs/DEPENDENCY_REGISTER.md.
# Usage: bash scripts/audit-dependency-register.sh   (from repo root)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
REGISTER=docs/DEPENDENCY_REGISTER.md
[[ -f "$REGISTER" ]] || { echo "missing $REGISTER"; exit 1; }

# 1) direct dependency names from [dependencies]/[build-dependencies]/
#    [dev-dependencies] across the workspace, excluding path deps.
mapfile -t DEPS < <(
  for f in Cargo.toml crates/*/Cargo.toml; do
    [[ -f "$f" ]] || continue
    awk '
      /^\[(dependencies|build-dependencies|dev-dependencies)\]$/ { in_dep=1; next }
      /^\[/ { in_dep=0 }
      in_dep && /^[a-zA-Z0-9_-]+[[:space:]]*=/ && $0 !~ /path[[:space:]]*=/ {
        name=$1; gsub(/[^a-zA-Z0-9_-]/,"",name); print name
      }
    ' "$f"
  done | sort -u
)

# 2) registered crate names from the register table (first cell of each row),
#    handling multi-name cells like "tonic / prost"; skip the path-crate row.
mapfile -t REGISTERED < <(
  awk -F'|' '
    /^\| Crate \| Tier \|/ { in_dep_table=1; next }
    in_dep_table && /workspace path/ { in_dep_table=0; next }
    in_dep_table && /^(\|[- ]+\|)+/ { next }
    in_dep_table && /^\|/ {
      cell=$2; gsub(/`/,"",cell); gsub(/^ +| +$/,"",cell);
      if (cell != "") { gsub(/[[:space:]]*\/[[:space:]]*/,"\n",cell); print cell }
    }
  ' "$REGISTER" | sort -u
)

missing=()
for dep in "${DEPS[@]}"; do
  if ! printf '%s\n' "${REGISTERED[@]}" | grep -qx "$dep"; then
    missing+=("$dep")
  fi
done

if [[ ${#missing[@]} -gt 0 ]]; then
  echo "UNREGISTERED direct dependencies (add a row to $REGISTER):"
  printf '  - %s\n' "${missing[@]}"
  exit 1
fi

stale=()
for reg in "${REGISTERED[@]}"; do
  if ! printf '%s\n' "${DEPS[@]}" | grep -qx "$reg"; then
    stale+=("$reg")
  fi
done
if [[ ${#stale[@]} -gt 0 ]]; then
  echo "REGISTERED but not a direct dependency (review, may be stale):"
  printf '  - %s\n' "${stale[@]}"
fi

echo "dependency-register audit OK ($((${#DEPS[@]})) direct deps, all registered)"
