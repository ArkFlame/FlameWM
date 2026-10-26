#!/usr/bin/env bash
set -euo pipefail
sudo pacman -S --needed \
  base-devel git rustup pkgconf python namcap \
  libx11 libxi libxft libxrender libxext libxcursor libxcomposite fontconfig dbus libpulse pam \
  libinput libdrm mesa libglvnd systemd-libs libxkbcommon wayland xorg-xwayland \
  xorg-server-xephyr xorg-server-xvfb xorg-xdpyinfo xorg-xwininfo xorg-xprop xorg-xrandr xorg-xwd \
  xorg-xclock xterm unzip imagemagick inkscape
rustup toolchain install stable --profile minimal
rustup component add rustfmt clippy
