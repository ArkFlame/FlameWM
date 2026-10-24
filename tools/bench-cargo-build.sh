#!/usr/bin/env bash
# Timed cargo build with a retained evidence record: wall, max RSS, user/sys, exit code,
# profile, jobs, target dir, toolchain versions and a unique run id.
# Usage: tools/bench-cargo-build.sh [--profile NAME] [--jobs N] [--target-dir DIR] [--label TEXT] [-- CARGO_ARGS...]
# Default cargo args: build --profile NAME --workspace --locked
# Refuses (exit 3) when another cargo/rustc already uses the same target dir.
# Evidence: <LOG_DIR>/<run-id>.log (cargo output) and <LOG_DIR>/runs.tsv (one row per run).
set -uo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd -P)"
PROFILE=fast
JOBS=${CARGO_BUILD_JOBS:-6}
TARGET_DIR=${CARGO_TARGET_DIR:-/run/media/linsaftw/SDD/flamewm-targets/bench}
LABEL=
LOG_DIR=${FLAMEWM_BENCH_LOG_DIR:-/run/media/linsaftw/SDD/flamewm-targets/bench-logs}
CARGO_ARGS=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile) PROFILE=$2; shift 2 ;;
    --jobs) JOBS=$2; shift 2 ;;
    --target-dir) TARGET_DIR=$2; shift 2 ;;
    --label) LABEL=$2; shift 2 ;;
    --) shift; CARGO_ARGS=("$@"); break ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "bench-cargo-build: unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [ "${#CARGO_ARGS[@]}" -eq 0 ]; then
  CARGO_ARGS=(build --profile "$PROFILE" --workspace --locked)
fi
case "$JOBS" in ''|*[!0-9]*) echo "bench-cargo-build: --jobs must be a positive integer" >&2; exit 2 ;; esac
[ "$JOBS" -ge 1 ] || { echo "bench-cargo-build: --jobs must be >= 1" >&2; exit 2; }

mkdir -p "$TARGET_DIR" "$LOG_DIR" || exit 2
TARGET_DIR=$(readlink -f "$TARGET_DIR")

# Refuse when a cargo or rustc process already targets this directory: concurrent
# builds would contend for the lock and corrupt timing evidence.
busy=()
for proc in /proc/[0-9]*; do
  comm=$(cat "$proc/comm" 2>/dev/null) || continue
  case "$comm" in cargo|rustc|rustc-*) ;; *) continue ;; esac
  if { tr '\0' '\n' <"$proc/environ"; } 2>/dev/null | grep -qx "CARGO_TARGET_DIR=$TARGET_DIR" \
     || { tr '\0' ' ' <"$proc/cmdline"; } 2>/dev/null | grep -qF -- "$TARGET_DIR"; then
    busy+=("${proc#/proc/}:$comm")
  fi
done
if [ "${#busy[@]}" -gt 0 ]; then
  echo "bench-cargo-build: refusing to start; active cargo/rustc on $TARGET_DIR: ${busy[*]}" >&2
  exit 3
fi

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$-$RANDOM"
LOG="$LOG_DIR/$RUN_ID.log"
TIMEFILE="$LOG_DIR/$RUN_ID.time"
CARGO_VERSION=$(cargo --version 2>/dev/null || echo unavailable)
RUSTC_VERSION=$(rustc --version 2>/dev/null || echo unavailable)

cd "$ROOT" || exit 2
CARGO_TARGET_DIR="$TARGET_DIR" CARGO_BUILD_JOBS="$JOBS" \
  /usr/bin/time -o "$TIMEFILE" -f '%e %U %S %M %x' \
  cargo "${CARGO_ARGS[@]}" >"$LOG" 2>&1
RC=$?

read -r WALL USER SYS RSS_KB TIME_RC < <(tail -n 1 "$TIMEFILE" 2>/dev/null)
[ -n "${WALL:-}" ] || { WALL=NA; USER=NA; SYS=NA; RSS_KB=NA; }
RSS_MB=NA
[ "$RSS_KB" != NA ] && RSS_MB=$(( RSS_KB / 1024 ))
CMD="cargo ${CARGO_ARGS[*]}"
HEADER=$'run_id\tlabel\tprofile\tjobs\ttarget_dir\twall_s\tuser_s\tsys_s\tmax_rss_mb\texit\tcargo\trustc\tcommand'
[ -f "$LOG_DIR/runs.tsv" ] || printf '%s\n' "$HEADER" >"$LOG_DIR/runs.tsv"
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$RUN_ID" "${LABEL:-none}" "$PROFILE" "$JOBS" "$TARGET_DIR" "$WALL" "$USER" "$SYS" \
  "$RSS_MB" "$RC" "$CARGO_VERSION" "$RUSTC_VERSION" "$CMD" >>"$LOG_DIR/runs.tsv"

printf 'run_id=%s\nlabel=%s\nprofile=%s jobs=%s target_dir=%s\n' "$RUN_ID" "${LABEL:-none}" "$PROFILE" "$JOBS" "$TARGET_DIR"
printf 'cargo=%s\nrustc=%s\ncommand=%s\n' "$CARGO_VERSION" "$RUSTC_VERSION" "$CMD"
printf 'wall_s=%s user_s=%s sys_s=%s max_rss_mb=%s exit=%s\n' "$WALL" "$USER" "$SYS" "$RSS_MB" "$RC"
printf 'log=%s\n' "$LOG"
exit "$RC"
