#!/usr/bin/env bash
# Reproducible explorer directory-listing bench: 20k-file fixture, release build.
# Prints key=value metrics from crates/flamewm-explorer/examples/dir_bench.rs.
# Usage: scripts/perf/explorer_dir.sh [fixture_dir] [file_count] [iterations]
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
FIXTURE="${1:-${FLAMEWM_PERF_FIXTURES:-/run/media/linsaftw/SDD/perf-fixtures}/explorer-dir}"
COUNT="${2:-20000}"
ITERATIONS="${3:-7}"
TARGET="${CARGO_TARGET_DIR:-/run/media/linsaftw/SDD/target-perfdefer}"

CARGO_TARGET_DIR="$TARGET" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
  cargo build --release -q -p flamewm-explorer --example dir_bench --manifest-path "$ROOT/Cargo.toml"

echo "fixture=$FIXTURE"
echo "load1=$(cut -d' ' -f1 /proc/loadavg)"
"$TARGET/release/examples/dir_bench" "$FIXTURE" "$COUNT" "$ITERATIONS"
