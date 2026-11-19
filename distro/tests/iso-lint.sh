#!/usr/bin/env bash
# iso-lint.sh [ISO] - static checks on a FlameDE ISO without booting it.
# Needs: isoinfo (genisoimage/cdrkit), unsquashfs (squashfs-tools), sha256sum, awk.
# Exit: 0 all checks pass, 1 a check failed, 2 no ISO / missing tool.
# shellcheck disable=SC2015  # pass/fail always return 0; A && B || C is intentional here
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
OUT_DIR=${OUT_DIR:-$REPO/distro/out}
FLAMEWM_BIN_RE=${FLAMEWM_BIN_RE:-'^/usr/(local/)?bin/flame-?wm[^/]*$'}

ISO=${1:-}
if [[ -z $ISO ]]; then shopt -s nullglob; isos=("$OUT_DIR"/FlameDE-*.iso); shopt -u nullglob; if (( ${#isos[@]} )); then ISO=${isos[-1]}; fi; fi
if [[ -z $ISO || ! -f $ISO ]]; then echo "FAIL no ISO found (arg or $OUT_DIR/FlameDE-*.iso)"; exit 2; fi
for t in isoinfo unsquashfs sha256sum awk; do
  command -v "$t" >/dev/null || { echo "FAIL missing tool: $t"; exit 2; }
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
failures=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; failures=$((failures + 1)); }

# 1. file, size, checksum sidecar
[[ -s $ISO ]] && pass "iso exists and non-empty: $ISO" || fail "iso empty: $ISO"
size_bytes=$(stat -c %s "$ISO")
printf 'INFO  size: %d MB\n' $((size_bytes / 1048576))
if [[ -f $ISO.sha256 ]]; then
  if (cd "$(dirname "$ISO")" && sha256sum --check --status "$(basename "$ISO").sha256"); then
    pass "sha256 matches $(basename "$ISO").sha256"
  else
    fail "sha256 mismatch against $ISO.sha256"
  fi
else
  fail "missing checksum sidecar $ISO.sha256 (run: sha256sum FILE > FILE.sha256)"
fi

# 2. ISO9660 / Rock Ridge listing
isoinfo -R -f -i "$ISO" 2>/dev/null | sed 's/;1$//' > "$WORK/iso.list"
if [[ ! -s $WORK/iso.list ]]; then
  fail "isoinfo could not list $ISO"
else
  for path in /casper/vmlinuz /casper/initrd /casper/filesystem.squashfs /boot/grub/grub.cfg; do
    grep -Fxq "$path" "$WORK/iso.list" && pass "iso has $path" || fail "iso missing $path"
  done
  grep -Eq '/efi\.img$' "$WORK/iso.list" && pass "iso has EFI boot image (*/efi.img)" \
    || fail "iso missing EFI boot image (*/efi.img)"
fi

# 3. squashfs root: extract only what is needed
if [[ -s $WORK/iso.list ]] && grep -Fxq /casper/filesystem.squashfs "$WORK/iso.list"; then
  SQ=$WORK/filesystem.squashfs
  isoinfo -R -i "$ISO" -x /casper/filesystem.squashfs > "$SQ"
  unsquashfs -no-progress -l "$SQ" | sed 's#^squashfs-root##' > "$WORK/fs.list"
  mkdir -p "$WORK/root/var/lib/dpkg"
  unsquashfs -cat "$SQ" var/lib/dpkg/status > "$WORK/root/var/lib/dpkg/status"
  awk '/^Package: /{p=$2} /^Status: .* installed$/{print p}' "$WORK/root/var/lib/dpkg/status" \
    | sort -u > "$WORK/installed.txt"

  # forbidden packages (exact names, plus lxqt*/lxde* families)
  forbidden=$(grep -Ex 'snapd|sddm|cloud-init|apport|whoopsie|ubuntu-advantage-tools|ubuntu-pro-client' \
    "$WORK/installed.txt" || true)
  forbidden+=$'\n'$(grep -E '^(lxqt|lxde)' "$WORK/installed.txt" || true)
  forbidden=$(printf '%s\n' "$forbidden" | sed '/^$/d')
  if [[ -z $forbidden ]]; then pass "no forbidden packages installed"
  else fail "forbidden packages installed: $(echo "$forbidden" | tr '\n' ' ')"; fi

  # forbidden files left behind
  for p in etc/cloud usr/lib/snapd var/lib/snapd usr/bin/snap usr/share/xsessions/lxqt.desktop; do
    if grep -Eq "^/$p(/|$)" "$WORK/fs.list"; then fail "forbidden path present: /$p"; fi
  done
  pass "no forbidden paths present (etc/cloud, snapd, lxqt session)"

  # required packages
  for pkg in lightdm network-manager dbus; do
    grep -Fxq "$pkg" "$WORK/installed.txt" && pass "package installed: $pkg" || fail "package missing: $pkg"
  done
  if grep -Eq '^(polkitd|policykit-1)$' "$WORK/installed.txt"; then pass "package installed: polkit"
  else fail "package missing: polkitd/policykit-1"; fi
  if grep -Eq '^(pipewire|pulseaudio)$' "$WORK/installed.txt"; then pass "audio server installed (pipewire or pulseaudio)"
  else fail "no audio server: need pipewire or pulseaudio"; fi

  # required files
  if grep -Eq "$FLAMEWM_BIN_RE" "$WORK/fs.list"; then pass "flamewm binary present"
  else fail "flamewm binary missing (pattern $FLAMEWM_BIN_RE)"; fi
  grep -Eq '^/usr/sbin/NetworkManager$' "$WORK/fs.list" && pass "NetworkManager binary present" \
    || fail "NetworkManager binary missing"
fi

if (( failures == 0 )); then echo "RESULT PASS"; exit 0; fi
echo "RESULT FAIL ($failures)"; exit 1
