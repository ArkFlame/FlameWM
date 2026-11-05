# Source me: `. "$(dirname "$0")/env.sh"` (scripts/dev/*.sh do this).
# Shared Cargo defaults for shared, loaded machines. Every value can be overridden from the caller.
#   FLAME_TARGET_DIR  shared target dir (default: one dir for all workers, so deps build once)
#   CARGO_BUILD_JOBS  default 2
#   RUST_TEST_THREADS default 2
export CARGO_TARGET_DIR="${FLAME_TARGET_DIR:-${CARGO_TARGET_DIR:-/run/media/linsaftw/SDD/target-shared}}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
# Cargo commands run under low CPU and IO priority so they do not starve the desktop or other workers.
FLAME_NICE=(nice -n 10)
if command -v ionice >/dev/null 2>&1; then FLAME_NICE+=(ionice -c3); fi
export FLAME_NICE
