#!/usr/bin/env bash
# Fast deterministic counter benchmarks: scoped debug `cargo test` filters; each test prints BENCH k=v lines.
# Usage: tools/fast-bench.sh   (exit 1 if any suite fails)
set -uo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd -P)"
# One shared Cargo target for every suite: all four are debug test builds of the
# same workspace, so per-suite target dirs only recompiled common dependencies.
BASE=${FLAMEWM_TARGET_BASE:-/run/media/linsaftw/SDD/flamewm-targets}/fast-bench
export CARGO_TARGET_DIR="$BASE"
SUITES=(
  "flamewm-terminal bench_row_rebuilds"
  "flamewm-render-core bench_setter_revisions"
  "flamewm-desktop rubber_motion_matches"
  "flamewm-render-x11 bench_mask_cache"
)
start=$(date +%s%N)
rc_all=0
rows=""
for suite in "${SUITES[@]}"; do
  read -r crate filter <<<"$suite"
  out=$(cd "$ROOT" && cargo test -q -p "$crate" "$filter" -- --nocapture 2>&1)
  rc=$?
  [ "$rc" -ne 0 ] && rc_all=1 && printf '%s\n' "$out" | tail -n 30 >&2
  pairs=$(printf '%s\n' "$out" | grep -o 'BENCH .*' | sed 's/^BENCH //' | tr ' ' '\n' | grep '=' || true)
  rows+="$(printf '%s\n' "$pairs" | sed "s/^/$crate|/")"$'\n'
  printf '%-22s exit=%s\n' "$crate" "$rc" >&2
done
end=$(date +%s%N)
printf '%-24s %-48s %s\n' SUITE COUNTER VALUE
printf '%s' "$rows" | grep = | while IFS='|' read -r crate kv; do
  printf '%-24s %-48s %s\n' "${crate#flamewm-}" "${kv%%=*}" "${kv#*=}"
done
printf "wall_ms=%d exit=%s\n" "$(( (end - start) / 1000000 ))" "$rc_all"
exit "$rc_all"
