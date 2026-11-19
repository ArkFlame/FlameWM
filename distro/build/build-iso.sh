#!/usr/bin/env bash
# FlameDE live ISO build. Runs INSIDE the builder container (distro/build/Containerfile).
# Inputs (read-only): /src/distro (config, overlay, build/repo). Outputs: /out. Scratch: /work.
# Fail-closed: any error aborts before an ISO is published.
#
# Mode notes: MMDEB_MODE=unshare (default) needs unprivileged user namespaces in the
# container (host run passes seccomp/apparmor=unconfined). MMDEB_MODE=fakechroot is the
# fallback when unshare is unavailable; it is slower and less faithful to file ownership.
set -Eeuo pipefail
trap 'printf "FLAMEDE-ISO-ERROR: line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&2' ERR

fail() { printf 'FLAMEDE-ISO-ERROR: %s\n' "$1" >&2; exit 1; }

SRC=${SRC:-/src/distro}
OUT=${OUT:-/out}
WORK=${WORK:-/work}
VERSION=${VERSION:-dev}
ARCH=amd64
# shellcheck source=/dev/null
SUITE=${SUITE:-$(. /etc/os-release && printf '%s' "$UBUNTU_CODENAME")}
MIRROR=${MIRROR:-http://archive.ubuntu.com/ubuntu}
COMPONENTS=${COMPONENTS:-main,universe,restricted,multiverse}
MMDEB_MODE=${MMDEB_MODE:-unshare}          # unshare | fakechroot
SQUASH_COMP=${SQUASH_COMP:-zstd}           # zstd | xz
ZSTD_LEVEL=${ZSTD_LEVEL:-19}
INITRAMFS_MODULES=${INITRAMFS_MODULES:-most} # most = safe default; dep = smaller, drops unknown-hardware drivers
VOLID=${VOLID:-FLAMEDE}

TEST_PROBE=${TEST_PROBE:-0}
case $TEST_PROBE in 0|1) ;; *) echo "FLAMEDE-ISO-ERROR: TEST_PROBE must be 0 or 1" >&2; exit 1 ;; esac
TEST_TAG=""
[[ "$TEST_PROBE" == 1 ]] && TEST_TAG="-test"
ISO_NAME="FlameDE-${VERSION}${TEST_TAG}-${ARCH}.iso"
ROOTFS="$WORK/rootfs"
ISODIR="$WORK/iso"
LOCALREPO="$WORK/localrepo"
PKG="$SRC/config/packages"
SYSCFG="$SRC/config/system"
OVERLAY="$SRC/overlay"

case $MMDEB_MODE in unshare|fakechroot) ;; *) fail "invalid MMDEB_MODE=$MMDEB_MODE" ;; esac
case $SQUASH_COMP in zstd|xz) ;; *) fail "invalid SQUASH_COMP=$SQUASH_COMP" ;; esac
case $INITRAMFS_MODULES in most|dep|list|netboot) ;; *) fail "invalid INITRAMFS_MODULES=$INITRAMFS_MODULES" ;; esac
[[ "$(dpkg --print-architecture)" == "$ARCH" ]] || fail "builder must be $ARCH (chroot runs native binaries)"
[[ -n "$SUITE" ]] || fail "could not determine SUITE"
for tool in mmdebstrap mksquashfs grub-mkrescue xorriso dpkg-scanpackages; do
    command -v "$tool" >/dev/null || fail "missing tool in builder: $tool"
done
[[ -d "$OUT" ]] || fail "output dir $OUT not mounted"

# Strip comments/blanks/whitespace from package lists; print comma-joined names.
read_lists() {
    local f out
    for f in "$@"; do
        [[ -s "$f" ]] || fail "missing or empty package list: $f"
    done
    out=$(cat "$@" | awk '{ sub(/#.*/, ""); gsub(/[ \t\r]/, ""); if (length) print }' | sort -u | paste -sd, -)
    [[ -n "$out" ]] || fail "no packages in: $*"
    printf '%s' "$out"
}
read_names() {  # newline-separated names
    [[ -s "$1" ]] || fail "missing or empty list: $1"
    awk '{ sub(/#.*/, ""); gsub(/[ \t\r]/, ""); if (length) print }' "$1" | sort -u
}

INCLUDE=$(read_lists "$PKG/base.list" "$PKG/desktop.list" "$PKG/live.list" "$PKG/installer.list")
REMOVE_NAMES="$WORK/remove.txt"
read_names "$PKG/remove.list" > "$REMOVE_NAMES"

# Local apt repo holding the flamewm .deb(s). Unsigned flat repo, used only inside the build.
shopt -s nullglob
debs=("$SRC/build/repo/"*.deb)
shopt -u nullglob
(( ${#debs[@]} > 0 )) || fail "no .deb in $SRC/build/repo (run distro/build/build.sh deb)"
flamewm_debs=0
for d in "${debs[@]}"; do
    arch=$(dpkg-deb -f "$d" Architecture) || fail "unreadable deb: $d"
    [[ "$arch" == "$ARCH" ]] || fail "deb $d has Architecture=$arch (expected $ARCH)"
    [[ "$(basename "$d")" == flamewm_* ]] && flamewm_debs=$((flamewm_debs + 1))
done
(( flamewm_debs >= 1 )) || fail "no flamewm_*.deb in $SRC/build/repo"

rm -rf "$ROOTFS" "$ISODIR" "$LOCALREPO"
mkdir -p "$LOCALREPO" "$ISODIR/casper" "$ISODIR/boot/grub"
cp -- "${debs[@]}" "$LOCALREPO/"
( cd "$LOCALREPO" && dpkg-scanpackages --multiversion . /dev/null > Packages && gzip -9kf Packages )

# ---- 1) bootstrap ----------------------------------------------------------
# Hook strings are intentionally single-quoted: mmdebstrap expands $1 itself.
# shellcheck disable=SC2016
mmdebstrap \
    --variant=minbase \
    --mode="$MMDEB_MODE" \
    --arch="$ARCH" \
    --components="$COMPONENTS" \
    --include="$INCLUDE" \
    --keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg \
    --essential-hook='copy-in /work/localrepo /opt/' \
    --essential-hook='echo "deb [trusted=yes] file:/opt/localrepo ./" > "$1/etc/apt/sources.list.d/flamewm-local.list"' \
    --customize-hook='chroot "$1" sh -c "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get -y install flamewm"' \
    --customize-hook='copy-in /work/remove.txt /root/' \
    --customize-hook='chroot "$1" sh -c "xargs -r apt-get -y purge < /root/remove.txt && rm -f /root/remove.txt"' \
    "$SUITE" "$ROOTFS" "$MIRROR"

[[ -x "$ROOTFS/usr/bin/zstd" ]] || fail "zstd missing in rootfs (needed for initramfs COMPRESS=zstd); add it to base.list"
[[ -f "$ROOTFS/usr/share/initramfs-tools/scripts/casper" ]] || fail "casper missing in rootfs; add it to live.list"
[[ -x "$ROOTFS/usr/bin/dpkg-query" ]] || fail "rootfs has no dpkg"

# ---- 2) overlays, masks, live user ------------------------------------------
# config/system subtrees are mapped explicitly (mask-list.txt is never copied).
# distro/overlay is copied verbatim: its paths are already rootfs-relative.
# Mapped sources are required: a missing one fails the build.
map_tree() { # src (under $SYSCFG) -> dest (absolute in rootfs)
    local src="$SYSCFG/$1" dst="$ROOTFS$2"
    if [[ -d "$src" ]]; then
        mkdir -p "$dst"
        ( cd "$src" && tar -cf - . ) | tar -C "$dst" -xf -
    elif [[ -f "$src" ]]; then
        install -D -m0644 "$src" "$dst"
    else
        fail "missing system config source: config/system/$1"
    fi
}
map_tree sysctl.d                    /etc/sysctl.d
map_tree systemd/journald.conf.d     /etc/systemd/journald.conf.d
map_tree systemd/system.conf.d       /etc/systemd/system.conf.d
map_tree systemd/zram-generator.conf /etc/systemd/zram-generator.conf
map_tree modprobe.d                  /etc/modprobe.d
map_tree udev/rules.d                /etc/udev/rules.d
map_tree lightdm/lightdm.conf.d      /etc/lightdm/lightdm.conf.d
if [[ -d "$OVERLAY" ]]; then
    ( cd "$OVERLAY" && tar -cf - . ) | tar -C "$ROOTFS" -xf -
fi

# Installer payload (distro/installer work): CLI + Calamares config.
[[ -f "$SRC/installer/flame-install" ]] || fail "missing installer/flame-install"
install -D -m0755 "$SRC/installer/flame-install" "$ROOTFS/usr/local/bin/flame-install"
[[ -d "$SRC/installer/calamares" ]] || fail "missing installer/calamares"
mkdir -p "$ROOTFS/etc/calamares"
( cd "$SRC/installer/calamares" && tar -cf - . ) | tar -C "$ROOTFS/etc/calamares" -xf -

# Test-only guest probe. Only when TEST_PROBE=1; such ISOs are tagged "-test".
if [[ "${TEST_PROBE:-0}" == 1 ]]; then
    [[ -f "$SRC/tests/guest/flame-probe.sh" ]] || fail "TEST_PROBE=1 but tests/guest/flame-probe.sh missing"
    [[ -f "$SRC/tests/guest/flame-probe.service" ]] || fail "TEST_PROBE=1 but tests/guest/flame-probe.service missing"
    install -D -m0755 "$SRC/tests/guest/flame-probe.sh" "$ROOTFS/usr/lib/flame-test/flame-probe.sh"
    install -D -m0644 "$SRC/tests/guest/flame-probe.service" "$ROOTFS/etc/systemd/system/flame-probe.service"
    mkdir -p "$ROOTFS/etc/systemd/system/multi-user.target.wants"
    ln -sfn ../flame-probe.service "$ROOTFS/etc/systemd/system/multi-user.target.wants/flame-probe.service"
fi

MASKS="$SYSCFG/mask-list.txt"
if [[ -s "$MASKS" ]]; then
    mkdir -p "$ROOTFS/etc/systemd/system"
    while IFS= read -r unit; do
        [[ -n "$unit" ]] || continue
        [[ "$unit" == */* ]] && fail "mask entry must be a unit name, not a path: $unit"
        ln -sfn /dev/null "$ROOTFS/etc/systemd/system/$unit"
    done < <(awk '{ sub(/#.*/, ""); gsub(/[ \t\r]/, ""); if (length) print }' "$MASKS")
fi

# Mount proc and dev only while chroot steps run; always unmount on exit.
umount_chroot() {
    local m
    for m in dev proc; do
        if mountpoint -q "$ROOTFS/$m"; then umount -l "$ROOTFS/$m"; fi
    done
}
trap 'umount_chroot' EXIT
mount -t proc proc "$ROOTFS/proc"
mount --bind /dev "$ROOTFS/dev"

chroot "$ROOTFS" env DEBIAN_FRONTEND=noninteractive sh -c '
    id -u flamelive >/dev/null 2>&1 || useradd -m -u 1000 -s /bin/bash \
        -G sudo,adm,audio,video,plugdev,netdev flamelive
    passwd -d flamelive
'

# ---- 3) initramfs (casper) ----------------------------------------------------
mkdir -p "$ROOTFS/etc/initramfs-tools/conf.d"
printf 'COMPRESS=zstd\nMODULES=%s\n' "$INITRAMFS_MODULES" > "$ROOTFS/etc/initramfs-tools/conf.d/flamede.conf"
chroot "$ROOTFS" update-initramfs -u -k all

# shellcheck disable=SC2012
KVER=$(ls -1 "$ROOTFS"/boot/vmlinuz-* 2>/dev/null | sed 's#.*/vmlinuz-##' | sort -V | tail -n1)
[[ -n "$KVER" ]] || fail "no kernel in rootfs; add linux-image-generic to base.list"
[[ -s "$ROOTFS/boot/initrd.img-$KVER" ]] || fail "initrd not produced for $KVER"
install -m0644 "$ROOTFS/boot/vmlinuz-$KVER" "$ISODIR/casper/vmlinuz"
install -m0644 "$ROOTFS/boot/initrd.img-$KVER" "$ISODIR/casper/initrd"
umount_chroot

# ---- 4) shrink -----------------------------------------------------------------
chroot "$ROOTFS" apt-get clean || true
rm -rf "$ROOTFS"/var/lib/apt/lists/* "$ROOTFS"/var/cache/apt/*.bin
find "$ROOTFS/var/cache/apt/archives" -name '*.deb' -delete 2>/dev/null || true
find "$ROOTFS/usr/share/doc" -mindepth 1 ! -name copyright ! -type d -delete 2>/dev/null || true
find "$ROOTFS/usr/share/doc" -mindepth 1 -type d -empty -delete 2>/dev/null || true
rm -rf "$ROOTFS"/usr/share/man "$ROOTFS"/usr/share/info "$ROOTFS"/usr/share/lintian \
       "$ROOTFS"/usr/share/bug "$ROOTFS"/usr/share/help
find "$ROOTFS/usr/share/locale" -mindepth 1 -maxdepth 1 ! -name 'en*' ! -name C -exec rm -rf {} + 2>/dev/null || true
rm -rf "$ROOTFS"/var/tmp/* "$ROOTFS"/tmp/* "$ROOTFS"/root/.cache
find "$ROOTFS/var/log" -type f -delete 2>/dev/null || true
rm -rf "$ROOTFS/opt/localrepo" "$ROOTFS/etc/apt/sources.list.d/flamewm-local.list"

# ---- 5) manifest, size, squashfs ------------------------------------------------
# shellcheck disable=SC2016
chroot "$ROOTFS" dpkg-query -W -f='${Package} ${Version}\n' > "$ISODIR/casper/filesystem.manifest"
du -sxB1 "$ROOTFS" | cut -f1 > "$ISODIR/casper/filesystem.size"

case $SQUASH_COMP in
    zstd) comp=(-comp zstd -Xcompression-level "$ZSTD_LEVEL") ;;
    xz)   comp=(-comp xz) ;;
esac
mksquashfs "$ROOTFS" "$ISODIR/casper/filesystem.squashfs" "${comp[@]}" -b 1M \
    -noappend -processors "$(nproc)" -wildcards \
    -e 'dev/*' -e 'proc/*' -e 'sys/*' -e 'run/*' -e 'tmp/*' -e 'var/tmp/*'

# ---- 6) GRUB BIOS+UEFI hybrid ISO ------------------------------------------------
cat > "$ISODIR/boot/grub/grub.cfg" <<'EOF'
set timeout=5
set default=0

menuentry "Try or Install FlameDE" {
    linux /casper/vmlinuz boot=casper quiet systemd.show_status=0 zswap.enabled=1 zram.num_devices=1 ---
    initrd /casper/initrd
}
EOF

TMP_ISO="$WORK/$ISO_NAME"
rm -f "$TMP_ISO"
grub-mkrescue -o "$TMP_ISO" "$ISODIR" -volid "$VOLID" >"$WORK/grub-mkrescue.log" 2>&1 \
    || { tail -n 40 "$WORK/grub-mkrescue.log" >&2; fail "grub-mkrescue failed"; }
[[ -s "$TMP_ISO" ]] || fail "empty ISO produced"

# ---- 7) publish + report ----------------------------------------------------------
install -m0644 "$TMP_ISO" "$OUT/$ISO_NAME"
( cd "$OUT" && sha256sum "$ISO_NAME" > "$ISO_NAME.sha256" )
iso_bytes=$(stat -c %s "$OUT/$ISO_NAME")
pkg_count=$(wc -l < "$ISODIR/casper/filesystem.manifest")
rootfs_bytes=$(cat "$ISODIR/casper/filesystem.size")
squash_bytes=$(stat -c %s "$ISODIR/casper/filesystem.squashfs")
printf 'FlameDE %s (%s, suite %s)\n' "$VERSION" "$ARCH" "$SUITE"
printf '  packages      %s\n' "$pkg_count"
printf '  kernel        %s\n' "$KVER"
printf '  rootfs        %s bytes\n' "$rootfs_bytes"
printf '  squashfs      %s bytes (%s, %s)\n' "$squash_bytes" "$SQUASH_COMP" "$INITRAMFS_MODULES"
printf '  iso           %s bytes\n' "$iso_bytes"
printf '  output        %s\n' "$OUT/$ISO_NAME"
cat "$OUT/$ISO_NAME.sha256"
