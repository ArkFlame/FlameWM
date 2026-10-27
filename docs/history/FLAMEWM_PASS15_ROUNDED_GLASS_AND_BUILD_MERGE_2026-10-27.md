# FlameWM Pass 15 — Rounded glass, X11 shadow corner, and Task Manager compiler repair

## Baseline and scope

Source: FlameWM 0.1.2 Pass 14. Separate coding-agent evidence: `command-code-session-ae4f265c.md` (user-supplied transcript, *not* a checked-out branch). Only the four source edits documented by that transcript were integrated, because no branch archive or commit was supplied.

## Source-grounded root causes

1. **Native Task Manager:** `Process` in `crates/flamewm-task-manager/src/model.rs` contains two `priority` fields and the parser initializes `priority` twice. The window resize handler passes `height: &u32` into `visible_rows(u32)`. This matches the separate agent's actual E0124/E0062/E0308 errors.
2. **Rounded titlebar contour:** `frame/chrome.rs::composite_surface_contour` previously started the top line at `x = radius`, then omitted both side arcs for rows above `radius`. The cut corners consequently had no bright rim.
3. **Taskbar glass outline:** `settings_projection::project` already sets the root border's color, but the four separate-mode capsules had neither dynamic `border(...)` projections nor compiled nonzero border widths. Shared tokens alone do not create stroke geometry.
4. **Bottom-right shadow notch:** `frame/shadow.rs::clipped_shadow_rows` clipped the shadow against the full rectangular frame, not its rounded X Shape bounding mask. A rounded client notch therefore exposed desktop pixels without a corresponding shadow cast. Compact rubber-band backdrop snapshots had the same square-cut assumption.
5. **Control hover:** `chrome.rs::paint_fill_behind` classified each pixel by its center and wrote an abrupt fully opaque disk. Its edge is now a 4x4 supersampled coverage ramp, generated on a bounded, cached glyph raster.
6. **Clipped round borders:** `app/render.rs` replaced a partially clipped round border with four square bands. The shared XRender ring painter now clips individual anti-aliased runs instead of losing curvature.

## Implementation and preservation

- `flamewm-ui-core::surface::inner_contour_coverage` is a renderer-neutral 1px rounded ring coverage helper. X11 frame chrome only blends that outline on the upper rim and the two titlebar sides; there is no artificial line at the internal titlebar bottom. Its color/opacity still come solely from Surfaces settings.
- The panel's compiled capsule CSS now establishes a 1px width; live style projection controls the stroke color/alpha and clears it in unified mode, or for invisible groups. No new configuration keys, caches or process boundaries.
- `rounded_shadow_mask_rectangles`, `shade_visible_shadow_rounded` and rounded `ShadowBands` use the existing X11 shadow-owner path. The classic zero-radius functions remain for compatibility and pure test comparisons. A shadow radius/style change causes a fresh mask, even if the frame dimensions are unchanged. Selection damage uses conservative updates in the narrow widened corner band to avoid exposing stale backdrop pixels.
- Existing X11 cursor, desktop selection geometry, window move/resize logic, window buttons' click targets, Wayland renderer and client protocol semantics are unchanged. No new dependencies added. The independent AA work occurs only when affected surfaces are painted, and control glyphs remain cached.

## Verification / limits

- Python checks: `tools/test_flamewm_pass15_contours.py` (5/5), `tools/test_shared_surface_dependency.py` (4/4), `tools/test_task_manager_rwr_css.py` (5/5), `tools/test_flamewm_task_manager_contract.py` (5/5). Total **19/19**.
- Independent randomized reference geometry: 5,000 dimensions × 5 rows = 25,000 symmetry/bounds checks. This is a model-level check, not native renderer proof.
- `test_flamewm_surface_icon_slide_regressions.py`: 3 failures on Pass 15 and the same 3 failures on untouched Pass 14, concerning stale theme-picker/backdrop source-pattern assertions unrelated to this patch. Tests were not weakened.
- Rust unit tests added for circle antialiasing, titlebar contour, rounded notch and clipped XRender ring. **Not executed:** Cargo/rustc unavailable. No live X11/Wayland screenshots, pixel comparison, or PSS/CPU measurements were possible.
- Anticipated resource impact: no idle polling, no additional GPU allocations; small additional CPU only during affected redraws from edge antialiasing. Compact shadow snapshots retain up to one extra frame-radius-wide band to support transparent corner notches; this can slightly raise memory use for floating frames.

## Workstation merge gate

```bash
cargo check --locked -p flamewm-task-manager -p flamewm-wm-x11 -p flamewm-shell -p flamewm-render-x11 -p flamewm-ui-core
cargo test --locked -p flamewm-task-manager -p flamewm-wm-x11 -p flamewm-shell -p flamewm-render-x11 -p flamewm-ui-core
./xephyr
```

Visually verify rounded window corners and the bottom-right shadow notch at 100%, 125%, and 200% scale; minimize/maximize/close hover in circle and square styles; taskbar Separate and Unified modes; full-panel contour; Start and other popups. Confirm settings updates for zero and full contour opacity, disabled shadows and squared decorations. Record CPU/PSS including Xorg and before/after screenshots. **Do not treat this archive as production-validated until the native checks pass.**
