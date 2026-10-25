# Contributing

Read `README.md` and `docs/DEVELOPMENT.md` before changing code. Keep changes scoped to the owning crate or documented integration boundary.

Use Rust toolchain `1.88.0`. Before opening a pull request, run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Do not commit generated build output, local runtime coordination state, credentials, or host-specific configuration. Explain behavior changes and verification in pull requests.

## Wayland session changes

The Wayland session (`crates/flamewm-wm-wayland`, wrapper `scripts/flamewm-wayland-session`, session entry `packaging/flamewm-wayland.desktop`) is preliminary. Read `docs/WAYLAND_PLAN.md` before touching it, keep the X11 session the default and supported path, and keep the wrapper's `--capabilities` gate (`drm_compiled=true`) intact.

Additional checks for Wayland changes, none of which proves a hardware session:

```bash
cargo check -p flamewm-wm-wayland --features drm
cargo test -p flamewm-wm-wayland --features drm
./scripts/tests/wayland-session-matrix.sh
```

A nested run, a unit test or a successful package build is not evidence that the native DRM session works. No successful hardware login is on record; describe the session as preliminary and do not claim hardware validation you did not perform.

