FROM debian:12
ENV DEBIAN_FRONTEND=noninteractive RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config curl ca-certificates git python3 python3-pip dpkg-dev fakeroot xz-utils zip tar gzip file \
    libinput-dev libgbm-dev libdrm-dev libudev-dev libx11-dev libdbus-1-dev libpulse-dev libxkbcommon-dev libxft-dev libfontconfig1-dev libegl1-mesa-dev libgles2-mesa-dev libpam0g-dev libxi-dev libssl-dev xwayland \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.88.0
RUN rustup component add rustfmt clippy && chmod -R a+rwX /opt/rustup /opt/cargo
