# FlameWM Pass 43 — shell library/binary API visibility

## Failure

`cargo check --workspace --all-targets` fails in the `flamewm-shell` binary with three E0624 errors:

- `crates/flamewm-shell/src/app.rs:420` calls `ShellRuntime::select_panel_output`.
- `crates/flamewm-shell/src/event_router.rs:176` calls `ShellSurfaces::select_input_output`.
- `crates/flamewm-shell/src/event_router.rs:177` calls `ShellRuntime::select_panel_output`.

Both methods were `pub(crate)` in the **library** crate (`src/lib.rs` exposes `pub mod runtime`). The binary (`src/main.rs`) is a separate Rust crate even though both targets belong to the same Cargo package. `pub(crate)` is therefore not visible from the binary.

## Change

- Expose only `ShellRuntime::select_panel_output` and `ShellSurfaces::select_input_output` as `pub`.
- Keep their actual implementations, validated output identity, and input-surface routing unchanged.
- Add `crates/flamewm-shell/tests/public_panel_routing.rs`, an external integration test resolving both methods to catch future accidental visibility regressions.

All other Pass 42 screenshot integrations and Pass 41 desktop fixes are preserved.

## Commands in the user's complete checkout

```bash
CARGO_BUILD_JOBS=2 cargo check -p flamewm-shell --all-targets
CARGO_BUILD_JOBS=2 cargo test -p flamewm-shell --test public_panel_routing
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 ./scripts/build-deb
./scripts/xephyr
```

Extract with GNU tar `--touch` to avoid reproducing the prior stale-Cargo-target timestamp issue. Do not reapply Pass 41/42 afterward.

## Scope and limitations

This is an integrated **source overlay**, not a standalone asset-complete source release. Extract onto a complete FlameWM checkout with the original `assets/` and `.vendor/` trees. Source/archive checks ran in the handoff environment; native Cargo and Xephyr execution must be verified on Linux where Rust and X11 are installed. No evidence yet establishes that the screenshot feature or all multi-monitor UX is functional in a live session.
