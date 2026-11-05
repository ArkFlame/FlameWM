#!/usr/bin/env bash
# Run unit tests of named crates on the `fast` profile with the shared target dir.
# Usage: scripts/dev/test-fast.sh <crate> [<crate> ...] [-- <test-filter> args]
# Example: scripts/dev/test-fast.sh flamewm-wm-wayland -- startup
set -euo pipefail
ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/../.." && pwd -P)"
. "$ROOT/scripts/dev/env.sh"
cd "$ROOT"
crates=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do crates+=(-p "$1"); shift; done
[ "${1:-}" = "--" ] && shift
if [ ${#crates[@]} -eq 0 ]; then echo "usage: $0 <crate>... [-- test-args]" >&2; exit 2; fi
exec "${FLAME_NICE[@]}" cargo test --profile fast "${crates[@]}" -- "$@"
