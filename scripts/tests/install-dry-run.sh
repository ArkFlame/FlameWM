#!/bin/sh
# Dry-run checks for install.sh: plan lines per distro, unknown distro refused, nothing written.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
INSTALL=$ROOT/install.sh
WORK=$(mktemp -d "${TMPDIR:-/tmp}/flamewm-install-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

for id in arch debian fedora unknownx; do
    printf 'NAME="test"\nID=%s\n' "$id" >"$WORK/os-$id"
done

# plan DISTRO ARGS...: run install.sh --dry-run against a fake os-release, inside WORK.
plan() {
    id=$1
    shift
    FLAMEWM_OS_RELEASE=$WORK/os-$id TMPDIR=$WORK/tmp sh "$INSTALL" --dry-run --prefix "$WORK/prefix" "$@"
}

# expect TEXT OUTPUT: fail unless OUTPUT contains TEXT.
expect() {
    case $2 in
        *"$1"*) ;;
        *) printf '%s\n' "$2" >&2; fail "missing plan line: $1" ;;
    esac
}

before=$(find "$WORK" | sort)

out=$(plan arch --ref main) || fail "arch dry-run exited non-zero"
expect "distribution arch -> pacman" "$out"
expect "pacman -S --needed" "$out"
expect "artifact: source (main)" "$out"
expect "cargo build --release --workspace --locked --features flamewm-wm-wayland/drm" "$out"

out=$(plan debian --ref main) || fail "debian dry-run exited non-zero"
expect "distribution debian -> apt" "$out"
expect "apt-get install -y" "$out"
expect "artifact: source (main)" "$out"

out=$(plan debian --ref v1.2.3) || fail "debian release dry-run exited non-zero"
expect "artifact: release (v1.2.3)" "$out"
expect "releases/download/v1.2.3/flamewm_1.2.3_amd64.deb" "$out"

out=$(plan fedora --ref main) || fail "fedora dry-run exited non-zero"
expect "distribution fedora -> dnf" "$out"
expect "dnf install -y" "$out"

if out=$(plan unknownx --ref main 2>&1); then
    fail "unknown distribution was accepted"
fi
expect "unsupported distribution" "$out"

if sh "$INSTALL" --bogus >/dev/null 2>&1; then
    fail "unknown argument was accepted"
fi

after=$(find "$WORK" | sort)
[ "$before" = "$after" ] || fail "--dry-run changed the filesystem"
[ ! -e "$WORK/prefix" ] || fail "--dry-run created the prefix"
[ ! -e "$WORK/tmp" ] || fail "--dry-run created the temp dir"

if command -v shellcheck >/dev/null 2>&1; then
    shellcheck "$INSTALL" "$0"
fi

printf 'OK install.sh dry-run checks\n'
