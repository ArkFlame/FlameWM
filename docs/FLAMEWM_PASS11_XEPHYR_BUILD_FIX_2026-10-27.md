# FlameWM 0.1.2 — Pass 11 Xephyr build repair

## Source findings

The archive includes the actual definitions that the user's E0432/E0433/E0609/E0560 errors said were missing:

- `crates/flamewm-ui-core/src/lib.rs` exports `pub mod surface;` and the `surface.rs` module implements `surface_fill`, `surface_stroke`, `SurfaceFill`, and `blend_surface_over_rgb`.
- `crates/flamewm-ui-x11/src/lib.rs` exports `pub mod surface_style;`; `surface_style.rs` implements `surface_border`.
- `crates/flamewm-skin/src/recipes/decoration.rs` defines the `ChromeTokens` `surface_border_rgb` and `surface_border_alpha` fields and initializes both in `Default`.
- `scripts/xephyr` previously invalidated only desktop conversion crates. Restored archive timestamps can predate cached `.rlib` artifacts, causing incompatible compile-time definitions to be reused. This is a plausible explanation for the reported diagnostics, not a direct observation of the user's Cargo target directory.
- `ui/task-manager/task-manager.css` used unsupported property names (`border-top`, `border-bottom`, `border-left`, `text-align`) and unsupported CSS margin value `auto`. The strict `flamewm-render-compiler` rejects those forms, which directly caused the observed Task Manager build-script panic.

## Changes

- Replace unsupported CSS side-specific borders with the compiler's existing uniform `border` styling, and remove redundant text alignment. This slightly changes the separators' visual treatment while preserving the widget structure, row geometry, identifiers, actions, and input behavior.
- Replace `margin-left:auto` in the header with a flex-grow spacer; replace `margin-right:auto` in the operation bar with `flex-grow:1`.
- Add `tools/ensure_shared_surface_cargo.py`: hash the path crate manifests and Rust source plus embedded Task Manager HTML/CSS, verify Cargo resolves the exact checkout, then run targeted `cargo clean -p NAME --release` **only when the inputs differ**. Never clean the target directory wholesale or remove source files.
- Hook the preflight and invalidation into `scripts/xephyr` before the release build. Repeat runs with the same sources do not clean the packages again.
- Add targeted Python regression tests exercising initial cache invalidation, unchanged no-op, changed dependency/UI invalidation, bad metadata and absent source, simulated failed clean, CSS property compatibility and Xephyr hook ordering.

## Verification contract

```bash
python3 -I tools/test_task_manager_rwr_css.py -q
python3 -I tools/test_shared_surface_cargo.py -q
python3 -I tools/test_flamewm_task_manager_contract.py -q
cargo check --locked -p flamewm-ui-core -p flamewm-ui-x11 -p flamewm-skin -p flamewm-explorer -p flamewm-wm-x11 -p flamewm-task-manager
cargo test --locked -p flamewm-task-manager
./xephyr
```

Static and fake-Cargo tests do not prove a native build. The Xephyr hook invalidates affected release artifacts on its next execution. The compile-time sources and the actual user machine's target cache could not be compared directly here.
