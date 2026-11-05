#!/usr/bin/env bash
# Thumbnail scheduling A/B bench (same process): old FIFO policy vs pass-start cancel + in-view + wake coalescing.
# Prints key=value per scenario (median over iterations). Fixture: PNG files in one directory.
# Usage: scripts/perf/explorer_thumbs.sh [fixture_dir] [iterations]
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
FIXTURE="${1:-${FLAMEWM_PERF_FIXTURES:-/run/media/linsaftw/SDD/perf-fixtures}/thumb-bench}"
ITERATIONS="${2:-3}"
TARGET="${CARGO_TARGET_DIR:-/run/media/linsaftw/SDD/target-perfdefer}"

CARGO_TARGET_DIR="$TARGET" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
  cargo build --release -q -p flamewm-explorer --example thumb_bench --manifest-path "$ROOT/Cargo.toml"

"$TARGET/release/examples/thumb_bench" "$FIXTURE" "$ITERATIONS"
