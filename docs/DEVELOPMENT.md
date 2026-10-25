# Development

## Commands

```bash
./scripts/doctor         # dependency + source-boundary preflight
./scripts/check          # static audit + Cargo metadata/check/test + release build + UI compile/inspect
./scripts/xephyr         # FlameWM session in nested X11
./scripts/verify-visual  # default: Xephyr product canaries + root screenshot capture
./scripts/bench-memory   # PSS/RSS sampling for FlameWM-owned processes
./scripts/lint           # optional strict rustfmt + Clippy hygiene gate
```

For an existing-image comparison, use `scripts/verify-visual --reference` with `FLAMEWM_VISUAL_CURRENT` and `FLAMEWM_VISUAL_REFERENCE` set to image paths. This mode does not launch a session; metrics are advisory unless both comparison thresholds are set.

The current repository version is `[workspace.package].version` in `Cargo.toml`, printed by `scripts/version`. Historical references to 0.0.2 describe the earlier project layout.

## Wayland session work (preliminary)

The Wayland session is developed in `crates/flamewm-wm-wayland` against
[`WAYLAND_PLAN.md`](WAYLAND_PLAN.md). Useful commands:

```bash
cargo check -p flamewm-wm-wayland --features drm   # native backend compiles
cargo test -p flamewm-wm-wayland --features drm    # logind, frame planner, probe tables (no GPU needed)
./scripts/tests/wayland-session-matrix.sh          # wrapper backend/gate matrix (no display needed)
flamewm-wayland --backend=nested                   # nested winit window inside the current session
flamewm-wayland --capabilities                     # build facts only, no seat access
```

The native build needs the `libinput`, `gbm`, `libdrm` and `libudev` development files plus
`Xwayland`. Do not present a nested run, a unit test or a successful package build as evidence that
the native session works: no real hardware login is on record, the X11 session stays the supported
default, and the Wayland session must be described as preliminary in code comments, docs and commit
messages.

## Work on the correct owner

- X11/ICCCM/EWMH/window/workspace defect -> `crates/flamewm-wm`.
- Flame product/service/state policy -> Flame API/platform/core crate.
- Web presentation/layout/paint/hit-test defect -> the corresponding `flamewm-render-*` owner, preserving promoted 0.0.6 behavior until differential parity is protected.
- V8 interaction translation -> `crates/flamewm-shell` using the V8 prototype and the 0.0.6 native controller as exact behavioral oracles.
- Desktop filesystem semantics -> `flamewm-desktop-core` plus the eventual desktop surface/controller.
- Settings behavior -> `flamewm-settings-core` / platform control contract plus the Settings presentation.
- Audio/media/network -> integration core/reactor, event-driven.

## Porting rule

For a parity surface, do not redesign before matching it:

```text
COPY EXACT OWNER -> TRANSLATE -> WIRE -> COMPILE -> DIFFERENTIAL -> REFACTOR
```

For the independent FlameWM WM, historical IceWM/FlameWM implementations are edge-case and protocol references only. They are never active source dependencies.

## Red gate law

If `./scripts/check` fails, stop feature mutation and fix the first proven compiler/test failure, then rerun the same gate. Static audits are not substitutes for Cargo. If the target compiler was not executed, report the artifact as compiler-unverified rather than claiming runtime PASS.
