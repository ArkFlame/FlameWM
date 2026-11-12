#!/bin/sh
# FlameWM installer.
#   curl -fsSL https://raw.githubusercontent.com/ArkFlame/FlameWM/main/install.sh | sh
#   sh install.sh [--prefix DIR] [--ref TAG|BRANCH] [--dry-run] [--uninstall] [--yes] [-h]
#
# Debian/Ubuntu/Mint/Pop with a published release: installs the release .deb (checked against
# SHA256SUMS when published). Everything else: clones --ref, builds with the same cargo command as
# scripts/build-deb, and installs the files listed in packaging/install-manifest.txt.
# Both routes build a staging tree laid out like the .deb (usr/..., etc/...), then copy it to PREFIX.
# Plain POSIX sh. Prompts read /dev/tty; without a terminal the install runs as --yes.
set -eu

REPO=https://github.com/ArkFlame/FlameWM
API=https://api.github.com/repos/ArkFlame/FlameWM
OS_RELEASE=${FLAMEWM_OS_RELEASE:-/etc/os-release}
ARCH_DEPS='base-devel git rustup pkgconf python libx11 libxi libxft libxrender libxext libxcursor libxcomposite fontconfig dbus libpulse pam libinput libdrm mesa libglvnd systemd-libs libxkbcommon wayland xorg-xwayland'
APT_DEPS='build-essential pkg-config python3 git curl ca-certificates libx11-dev libxi-dev libxft-dev libxrender-dev libxext-dev libxcursor-dev libxcomposite-dev libfontconfig-dev libdbus-1-dev libpam0g-dev libpulse-dev libinput-dev libdrm-dev libgbm-dev libudev-dev libegl-dev libgles-dev libxkbcommon-dev libwayland-dev xwayland'
DNF_DEPS='gcc gcc-c++ make pkgconf-pkg-config python3 git curl ca-certificates libX11-devel libXi-devel libXft-devel libXrender-devel libXext-devel libXcursor-devel libXcomposite-devel fontconfig-devel dbus-devel pam-devel pulseaudio-libs-devel libinput-devel libdrm-devel mesa-libgbm-devel systemd-devel mesa-libEGL-devel mesa-libGLES-devel libxkbcommon-devel wayland-devel xorg-x11-server-Xwayland'

PREFIX=
REF=
DRY_RUN=0
UNINSTALL=0
YES=0
NEED_SUDO=0
PM=
TMP=
MANIFEST=

say() { printf 'flamewm-install: %s\n' "$*"; }
die() { printf 'flamewm-install: error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
usage: install.sh [--prefix DIR] [--ref TAG|BRANCH] [--dry-run] [--uninstall] [--yes] [-h]

  --prefix DIR   install root (default /usr/local as root, otherwise ~/.local)
  --ref REF      release tag or branch (default: latest release tag, else main)
  --dry-run      print every action and change nothing
  --uninstall    remove the files recorded in the install manifest under --prefix
  --yes          do not prompt (implied when there is no terminal)
  -h             show this help
EOF
}

# Every action goes through run: printed as "+ cmd", executed unless --dry-run.
run() {
    printf '+ %s\n' "$*"
    if [ "$DRY_RUN" = 0 ]; then "$@"; fi
}

# Files under PREFIX use sudo only when the nearest existing parent is not writable.
priv() {
    if [ "$NEED_SUDO" = 1 ]; then run sudo "$@"; else run "$@"; fi
}

# Package installs use sudo unless already root. The sudo prompt is printed by run first.
pkg() {
    if [ "$(id -u)" = 0 ]; then run "$@"; else say "sudo is needed to install packages"; run sudo "$@"; fi
}

need() {
    [ "$DRY_RUN" = 1 ] && return 0
    command -v "$1" >/dev/null 2>&1 || die "$1 not found ($2)"
}

detect_distro() {
    [ -r "$OS_RELEASE" ] || die "cannot read $OS_RELEASE"
    id=$(sed -n 's/^ID=//p' "$OS_RELEASE" | tr -d "\"'" | head -n 1)
    case $id in
        arch|manjaro|endeavouros) PM=pacman ;;
        debian|ubuntu|linuxmint|pop) PM=apt ;;
        fedora) PM=dnf ;;
        *)
            printf 'flamewm-install: unsupported distribution "%s". Build and install by hand:\n' "$id" >&2
            printf '  https://github.com/ArkFlame/FlameWM#build-from-source\n' >&2
            exit 1 ;;
    esac
    say "distribution $id -> $PM"
}

# The release .deb is used only for apt systems with a tag and a published .deb for this host.
use_release() {
    [ "$PM" = apt ] || return 1
    case $REF in v[0-9]*) ;; *) return 1 ;; esac
    [ "$DRY_RUN" = 1 ] && return 0
    [ "$(uname -m)" = x86_64 ] || return 1
    curl -fsSIL "$REPO/releases/download/$REF/$DEB" >/dev/null 2>&1
}

install_build_deps() {
    say "installing build dependencies with $PM"
    # shellcheck disable=SC2086
    case $PM in
        pacman) pkg pacman -S --needed --noconfirm $ARCH_DEPS ;;
        apt) pkg apt-get install -y $APT_DEPS ;;
        dnf) pkg dnf install -y $DNF_DEPS ;;
    esac
}

fetch_release() {
    base=$REPO/releases/download/$REF
    say "release artifact $DEB from $REF"
    run curl -fsSL -o "$TMP/$DEB" "$base/$DEB"
    if [ "$DRY_RUN" = 1 ]; then
        say "would verify $DEB against SHA256SUMS when published"
    elif curl -fsSL -o "$TMP/SHA256SUMS" "$base/SHA256SUMS" 2>/dev/null; then
        (cd "$TMP" && grep -F "  $DEB" SHA256SUMS | sha256sum -c -) || die "checksum verification failed for $DEB"
        say "checksum verified"
    else
        say "no SHA256SUMS published for $REF; $DEB is not checksum-verified"
    fi
}

install_deb_deps() {
    if [ "$DRY_RUN" = 1 ]; then say "+ apt-get install -y <Depends of $DEB>"; return; fi
    deps=$(dpkg-deb -f "$1" Depends | sed 's/([^)]*)//g; s/,/ /g')
    # shellcheck disable=SC2086
    pkg apt-get install -y $deps
}

build_checkout() {
    run git clone --depth 1 --branch "$REF" "$REPO.git" "$TMP/src"
    if [ "$DRY_RUN" = 0 ]; then cd "$TMP/src"; fi
    run env CARGO_TARGET_DIR="$TMP/target" cargo build --release --workspace --locked --features flamewm-wm-wayland/drm
}

# Copy each source listed in packaging/install-manifest.txt into the staging tree.
stage_checkout() {
    if [ "$DRY_RUN" = 1 ]; then
        say "+ stage the files listed in packaging/install-manifest.txt under $STAGE"
        return
    fi
    mkdir -p "$STAGE"
    while read -r from to; do
        case $from in ''|'#'*) continue ;; esac
        case $from in
            .target/release/*) from=$TMP/target/release/${from#.target/release/} ;;
            *) from=$TMP/src/$from ;;
        esac
        dest=$STAGE/$to
        case $to in
            */) mkdir -p "$dest"; cp -R "$from/." "$dest" ;;
            *) mkdir -p "$(dirname "$dest")"; cp "$from" "$dest" ;;
        esac
    done <"$TMP/src/packaging/install-manifest.txt"
}

# Copy the staging tree under PREFIX and record every written path in the manifest.
install_stage() {
    if [ "$DRY_RUN" = 1 ]; then
        say "+ install every file under $STAGE/usr to $PREFIX and record it in $MANIFEST"
        return
    fi
    if [ "$PREFIX" != /usr ]; then
        # The session .desktop entries hard-code /usr paths; point them at PREFIX.
        run find "$STAGE/usr/share" -name '*.desktop' -exec sed -i "s|/usr/|$PREFIX/|g" {} +
    fi
    find "$STAGE" \( -type f -o -type l \) >"$TMP/files"
    : >"$TMP/installed"
    while IFS= read -r f; do
        rel=${f#"$STAGE"/}
        case $rel in
            usr/*) dst=$PREFIX/${rel#usr/} ;;
            etc/*)
                if [ "$PREFIX" != /usr ]; then
                    say "skipped $rel: system config needs /etc and root; copy it by hand if you want it"
                    continue
                fi
                dst=/$rel ;;
            *) continue ;;
        esac
        mode=0644
        [ -x "$f" ] && mode=0755
        priv mkdir -p "$(dirname "$dst")"
        priv install -m "$mode" "$f" "$dst"
        printf '%s\n' "$dst" >>"$TMP/installed"
    done <"$TMP/files"
    priv mkdir -p "$(dirname "$MANIFEST")"
    priv install -m 0644 "$TMP/installed" "$MANIFEST"
    say "recorded $(wc -l <"$TMP/installed") files in $MANIFEST"
}

uninstall() {
    [ -f "$MANIFEST" ] || die "no install manifest at $MANIFEST; nothing to remove"
    while IFS= read -r path; do
        priv rm -f "$path"
    done <"$MANIFEST"
    priv rm -f "$MANIFEST"
    say "removed the files listed in $MANIFEST"
}

confirm() {
    [ "$YES" = 1 ] || [ "$DRY_RUN" = 1 ] && return 0
    printf 'Install FlameWM %s to %s? [y/N] ' "$REF" "$PREFIX" >/dev/tty
    read -r answer </dev/tty || answer=
    case $answer in y|Y|yes|YES) ;; *) die "aborted" ;; esac
}

main() {
    while [ $# -gt 0 ]; do
        case $1 in
            --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX=$2; shift 2 ;;
            --ref) [ $# -ge 2 ] || die "--ref needs a tag or branch"; REF=$2; shift 2 ;;
            --dry-run) DRY_RUN=1; shift ;;
            --uninstall) UNINSTALL=1; shift ;;
            --yes) YES=1; shift ;;
            -h|--help) usage; exit 0 ;;
            *) usage >&2; die "unknown argument: $1" ;;
        esac
    done

    detect_distro
    if [ -z "$PREFIX" ]; then
        if [ "$(id -u)" = 0 ]; then
            PREFIX=/usr/local
        else
            PREFIX=$HOME/.local
            say "not root: installing to $PREFIX (use --prefix to change)"
        fi
    fi
    case $PREFIX in /*) ;; *) die "--prefix must be an absolute path" ;; esac
    MANIFEST=$PREFIX/share/flamewm/installed-files.txt

    # Without a terminal there is nobody to ask, so run as --yes.
    if ! (: </dev/tty) 2>/dev/null; then YES=1; fi

    probe=$PREFIX
    while [ ! -d "$probe" ]; do probe=$(dirname "$probe"); done
    if [ ! -w "$probe" ]; then
        NEED_SUDO=1
        say "sudo will be used to write under $PREFIX"
    fi

    if [ "$UNINSTALL" = 1 ]; then
        confirm
        uninstall
        exit 0
    fi

    if [ -z "$REF" ]; then
        REF=$(curl -fsSL "$API/releases/latest" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) || REF=
        if [ -z "$REF" ]; then REF=main; fi
    fi
    case $REF in *[!A-Za-z0-9._/-]*|'') die "invalid ref: $REF" ;; esac
    DEB=flamewm_${REF#v}_amd64.deb

    if [ "$DRY_RUN" = 1 ]; then
        TMP=${TMPDIR:-/tmp}/flamewm-install.dry-run
    else
        TMP=$(mktemp -d "${TMPDIR:-/tmp}/flamewm-install.XXXXXX")
        trap 'rm -rf "$TMP"' EXIT
    fi
    STAGE=$TMP/stage

    if use_release; then
        say "artifact: release ($REF)"
        confirm
        need curl "needed to download the release"
        fetch_release
        install_deb_deps "$TMP/$DEB"
        run mkdir -p "$STAGE"
        run dpkg-deb -x "$TMP/$DEB" "$STAGE"
    else
        say "artifact: source ($REF)"
        confirm
        install_build_deps
        need git "install git"
        need cargo "install Rust with rustup (https://rustup.rs)"
        build_checkout
        stage_checkout
    fi
    install_stage

    say "done. Make sure $PREFIX/bin is on PATH; the session appears in your display manager."
    say "uninstall with: sh install.sh --uninstall --prefix $PREFIX"
}

main "$@"
