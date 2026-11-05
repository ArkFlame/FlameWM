#!/usr/bin/env bash
# Fast type-check of touched crates on the `fast` profile with the shared target dir.
# Usage: scripts/dev/check.sh <crate> [<crate> ...]      e.g. scripts/dev/check.sh flamewm-wm-wayland
# Without arguments it checks the whole workspace (slow; prefer naming crates).
set -euo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
. "$ROOT/scripts/dev/env.sh"
cd "$ROOT"
args=()
for c in "$@"; do args+=(-p "$c"); done
if [ ${#args[@]} -eq 0 ]; then args=(--workspace); fi
exec "${FLAME_NICE[@]}" cargo check --profile fast "${args[@]}"
