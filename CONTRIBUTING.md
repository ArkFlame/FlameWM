# Contributing

Read `README.md` and `docs/DEVELOPMENT.md` before changing code. Keep changes scoped to the owning crate or documented integration boundary.

Use Rust toolchain `1.88.0`. Before opening a pull request, run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Do not commit generated build output, local runtime coordination state, credentials, or host-specific configuration. Explain behavior changes and verification in pull requests; the pull request template lists what to include.

For visible changes, also run the headless visual checks. They need Xorg with `xf86-video-dummy`, `xdotool`, ImageMagick, `xterm` and `python-xlib`:

```bash
python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release --out .target/visual
```

## Where to start

Issues labelled `good first issue` are small and self-contained. If none are open, these are good first changes:

- Add a mixed-resolution two-screen layout (for example 1920x1080 next to 1280x800) to `tools/flamewm_visual_tests.py`.
- Stop the nested Wayland compositor from logging an EGL `BAD_SURFACE` error on every frame under software GL.
- Advertise `wp-cursor-shape-v1` in `crates/flamewm-wm-wayland` so native clients can ask for named cursors.

## Wayland session changes

The Wayland session (`crates/flamewm-wm-wayland`, wrapper `scripts/flamewm-wayland-session`, session entry `packaging/flamewm-wayland.desktop`) has no hardware login on record yet. Read `docs/WAYLAND_PLAN.md` before touching it, keep the X11 session the default and supported path, and keep the wrapper's `--capabilities` gate (`drm_compiled=true`) intact.

Additional checks for Wayland changes, none of which proves a hardware session:

```bash
cargo check -p flamewm-wm-wayland --features drm
cargo test -p flamewm-wm-wayland --features drm
./scripts/tests/wayland-session-matrix.sh
```

A nested run, a unit test or a successful package build is not evidence that the native DRM session works. No successful hardware login is on record; state what you ran and do not claim hardware validation you did not perform.

