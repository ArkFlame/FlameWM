# Cpak experimental application packaging

This is an **application-only** Cpak v3 build for FlameWM Terminal and
Flamefetch, not a full FlameWM desktop login session. It uses cpak's isolated
`displayX11` support; it does not receive the host's X11 display.

The native DRM/Wayland desktop uses raw logind system-bus calls. Cpak v3 has
no raw system bus permission, so advertising a working native host-login session
here would be misleading. Likewise, the full Shell expects desktop-service
D-Bus integration absent from the isolated package. Keep the existing .deb
and Arch packages for full sessions until supported integration is proven.

1. On a compatible Ubuntu 26.04 build host, build the normal FlameWM release
   first (with the required full source/assets and Rust toolchain).
2. Run `python3 tools/cpak_package.py prepare --output .target/cpak-context`.
   A second run needs another new directory; it never overwrites a prior build.
3. Run `podman build -f .target/cpak-context/Containerfile -t ghcr.io/arkflame/flamewm-terminal:0.1.3 .target/cpak-context`.
4. Test the container locally; push it to a registry only once satisfied. Record
   the **immutable digest from the registry**, not a local image id or mutable tag.
5. Generate the manifest with
   `python3 tools/cpak_package.py manifest --image ghcr.io/arkflame/flamewm-terminal@sha256:ACTUAL_DIGEST --output cpak.json`
   (replace ACTUAL_DIGEST with 64 hex digits).
6. `cpak validate cpak.json`, `cpak lock cpak.json` and
   `cpak test cpak.json --binary /usr/bin/flamefetch`.

The Cpak image is an ordinary OCI image; its packaging does not require FlameWM
runtime or compositor source changes. The generated manifest is intentionally
**not shipped** with a fabricated digest; publishing requires real registry
artifacts. `cpak` itself is not shipped or installed by this source patch.

The base image has glibc and native runtime libraries; binaries built against
an incompatible glibc will fail to launch. For portable release artifacts,
build the native binaries inside the same base distribution or use controlled
CI rather than importing arbitrary local builds.
