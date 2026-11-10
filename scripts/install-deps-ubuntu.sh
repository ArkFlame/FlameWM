#!/usr/bin/env bash
set -euo pipefail
sudo apt update
sudo apt install -y \
  build-essential pkg-config curl ca-certificates python3 \
  libx11-dev libwayland-dev libdbus-1-dev libxft2 libfontconfig1 fontconfig \
  xserver-xephyr xvfb x11-utils x11-apps xterm \
  unzip imagemagick inkscape
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  export PATH="$HOME/.cargo/bin:$PATH"
fi
rustup toolchain install stable --profile minimal
rustup component add rustfmt clippy
