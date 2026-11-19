#!/usr/bin/env bash
# Host wrapper for the FlameDE ISO pipeline.
#   build.sh deb    ensure distro/build/repo holds flamewm_<version>_amd64.deb
#   build.sh image  build the builder container (distro/build/Containerfile)
#   build.sh iso    deb + image, then run build-iso.sh rootless in podman (default)
# Output: distro/out/FlameDE-<version>[-test]-amd64.iso (+ .sha256). TEST_PROBE=1 adds the guest probe.
set -Eeuo pipefail

HERE="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd -P)"
DISTRO="$(dirname "$HERE")"
ROOT="$(dirname "$DISTRO")"

fail() { printf 'FLAMEDE-BUILD-ERROR: %s\n' "$1" >&2; exit 1; }

usage() { printf 'usage: %s [deb|image|iso]\n' "$0"; }

cmd="${1:-iso}"
case "$cmd" in
    deb|image|iso) ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac

ENGINE="${ENGINE:-podman}"
command -v "$ENGINE" >/dev/null 2>&1 || fail "$ENGINE not found"
VERSION="$("$ROOT/scripts/version")"
[[ -n "$VERSION" ]] || fail "scripts/version printed nothing"
UBUNTU_IMAGE="${UBUNTU_IMAGE:-ubuntu:26.10}"
IMAGE="${FLAMEDE_BUILDER_IMAGE:-flamede-builder:$VERSION}"
REPO="$HERE/repo"
OUT="$DISTRO/out"
TARGET="${CARGO_TARGET_DIR:-$ROOT/.target}"
[[ "$TARGET" == /* ]] || TARGET="$ROOT/$TARGET"
WORK="${FLAMEDE_WORK:-$ROOT/.target/distro/work}"
DEB_NAME="flamewm_${VERSION}_amd64.deb"

build_deb() {
    mkdir -p "$REPO"
    if [[ -s "$REPO/$DEB_NAME" ]]; then
        printf 'deb present: %s\n' "$REPO/$DEB_NAME"
        return
    fi
    local src="$TARGET/dist/$DEB_NAME"
    if [[ ! -s "$src" ]]; then
        "$ROOT/scripts/build-deb" || fail "scripts/build-deb failed"
    fi
    [[ -s "$src" ]] || fail "expected $src after build-deb"
    # Keep exactly one flamewm .deb in the repo so apt cannot pick a stale version.
    rm -f "$REPO"/flamewm_*.deb
    install -m0644 "$src" "$REPO/$DEB_NAME"
    printf 'deb copied: %s\n' "$REPO/$DEB_NAME"
}

build_image() {
    "$ENGINE" build --build-arg "UBUNTU_IMAGE=$UBUNTU_IMAGE" -t "$IMAGE" \
        -f "$HERE/Containerfile" "$HERE" || fail "image build failed"
}

build_iso() {
    build_deb
    build_image
    mkdir -p "$OUT" "$WORK"
    local -a env_args=(-e "VERSION=$VERSION")
    local v
    for v in SUITE MIRROR COMPONENTS MMDEB_MODE SQUASH_COMP ZSTD_LEVEL INITRAMFS_MODULES VOLID TEST_PROBE; do
        [[ -n "${!v+x}" ]] && env_args+=(-e "$v=${!v}")
    done
    # Rootless: bootstrap needs nested user namespaces (unshare mode) and chroot/mount
    # inside the container. seccomp/apparmor are relaxed for that reason only.
    "$ENGINE" run --rm \
        --name "flamede-build-$$" \
        --security-opt seccomp=unconfined \
        --security-opt apparmor=unconfined \
        --security-opt label=disable \
        --cap-add SYS_ADMIN \
        "${env_args[@]}" \
        -v "$DISTRO:/src/distro:ro" \
        -v "$WORK:/work" \
        -v "$OUT:/out" \
        "$IMAGE" bash /src/distro/build/build-iso.sh || fail "ISO build failed"
    printf "done: output in %s\n" "$OUT"
}

case "$cmd" in
    deb) build_deb ;;
    image) build_image ;;
    iso) build_iso ;;
esac
