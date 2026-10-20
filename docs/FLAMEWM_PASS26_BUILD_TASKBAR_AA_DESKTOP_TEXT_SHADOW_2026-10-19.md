# FlameWM Pass 26 — Rust all-targets compile fix, floating taskbar corner paint, desktop text shadow

## Observed failure

`cargo check --workspace --all-targets` emits ten `E0425` / `E0433` errors in `flamewm-render-x11/src/app/events.rs`, all inside the `reactor_presentation_regressions` test module. The module calls `desktop_takes_focus_on_press` and uses `X11WindowRole` without importing them from its parent module. This is **not** a Cargo stale-cache failure; normal production builds may compile while `--all-targets` fails. The fix imports both symbols into the test module. No production focus logic was removed.

## Floating taskbar antialiasing

The render path already contains an XRender anti-aliased rounded fill/stroke implementation (`xrender_aa.rs`). The native dock's XShape mask uses a floating taskbar corner radius, but the paint scene's `.taskbar` node had radius zero. Pass 26 projects `panel.corner_radius()` (physical px) back into logical CSS px via `panel_ui_scale()` before publishing the native dock frame. As a result, fill and stroke use the same geometric arc as the native mask; classic bars are set back to zero. The canonical `set_border_radius` setter now skips unchanged values to avoid recurring dirty redraws on panel projection. This removes the obvious square painted edge behind floating masks.

**Limitation:** XShape is binary, not per-pixel alpha. On a non-composited X11 desktop, fractional translucency across the physical window boundary cannot be guaranteed. This is a significant residual limitation; actual corner AA must be checked under the destination's X11 compositor/shape-backdrop environment. Do not claim compositor-grade fractional edge transparency based solely on the source checks.

## Desktop label shadow

Introduced a generic typed `text-shadow: <signed-x-px> <signed-y-px> <color>` CSS property in the strict compiler. The `.desktop-label-line` rule uses `0px 1px #000000b8` and inherits it onto actual glyph text nodes. Paint emits the offset shadow glyph before the normal glyph through the existing shared Xft painter. Drag-ghost/prediction labels inherit the same style. No global text shadows are applied to context-menu or other OS text.

This adds a compiled Style field: `FORMAT_VERSION=6`; decoder accepts v5 (defaulting to no shadow) and v2–v4. The inspector now reads and prints the file's actual wire version.

## Verification gates

- Offline regression: `python3 -I -m unittest discover -s tools -p 'test_flamewm_pass26_corner_shadow_compile.py' -v`.
- Focused package/dependency checks: `test_flamewm_pass25_cargo_recovery.py`, `test_flamewm_package_pipeline.py`, `test_package_source_archive.py`, `test_flamewm_pass24_regressions.py`, `test_flamewm_surface_icon_slide_regressions.py`, `test_desktop_conversion_cargo_gate.py`, and `test_flamewm_xtest_drag.py`.
- `python3 -I tools/ui_contract_guard.py` (0 errors; existing warnings).
- Native check on full repo: `CARGO_BUILD_JOBS=2 ./scripts/check-cargo` and `CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-core -p flamewm-render-compiler -p flamewm-render-x11 -p flamewm-shell` before `./scripts/build-deb`.

Compilation and Xephyr/Sddm runtime inspection are NOT_RUN in the source-only sandbox. Do not promote this release as native verified without executing those gates.
