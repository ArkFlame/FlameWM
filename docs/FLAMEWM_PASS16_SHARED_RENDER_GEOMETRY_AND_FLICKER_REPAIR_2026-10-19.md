# FlameWM Pass 16 — shared rendering-mask / animation-lifecycle repair

Date: 2026-10-19. Source baseline: Pass 15. **Native Rust compilation / Xephyr runtime were not available here**. This is a source implementation and offline-contract verification, not a certified runtime fix.

## Failure contracts addressed

1. Native popup slide-down: retained repaints could restore a full XShape bounding region during an active slide, exposing stale rounded corners until a later coordinate change. On completion, resetting a full shape before the X server had unmapped the window could expose one fully shaped flash over the panel.
2. Transition overlays for fade, scale and lamp: opaque no-compositor fallback paints a root snapshot across the transition's bounding window. If the window's XShape silhouette is not updated each frame, animated corners and uncovered bands reveal *frozen snapshot* or black rather than current underlying pixels.
3. Window resize: frame geometry changes occurred after titlebar chrome paint, leaving freshly enlarged frame areas as dark/black pixels; the interactive resize path deferred repaint and shape updates until release.
4. Window-open Scale: the WM formerly routed some Scale paths through a multi-band Magic Lamp warp rather than a single affine stretch. Direct ARGB window-open Fade also lacked the common skin-derived corner radius.
5. Maximize/demaximize: default enabled/"magic-lamp" label was inappropriate. The new default is **no maximize animation**; its opt-in scale runs at 110 ms with constant-speed geometry interpolation.
6. Unpainted client window-open captures: a colorful titlebar above uniform/unpainted client pixels was treated as ready. The capture gate now avoids starting this partial-content transition.

## Canonical sources modified

- `crates/flamewm-render-x11/src/surface_controller.rs`, `surface_controller/present.rs`: keep slide mask authoritative after retained redraws/Expose and on each active frame; XUnmap then XSync before resetting the full shape on slide-out completion.
- `crates/flamewm-render-x11/src/overlay.rs`, `xshape.rs`: shared render-owned dynamic bounding mask for every warp/stretch/fade/crossfade frame; coalesce identical adjacent row rectangles; mask starts empty on first native map. Shape initialization is mandatory for transition overlays; if unavailable, skip animation rather than silently expose a black/frozen fallback.
- `crates/flamewm-render-x11/src/surface_controller/slide.rs`: pass popup corner radius to overlay effects.
- `crates/flamewm-wm-x11/src/frame/controller.rs`: configure frame/client first, then measure/layout and paint chrome, then update frame shape, including during drag resize.
- `crates/flamewm-wm-x11/src/wm/overlay.rs`: stretch on open Scale; use identical skin radius for both general overlays and direct ARGB open-fade; opt-in maximize scale uses linear transform progress.
- `crates/flamewm-wm-x11/src/wm/paint.rs`: diagnostics when chrome rendering fails.
- `crates/flamewm-wm-x11/src/animation.rs`, `crates/flamewm-reactor/src/timeline.rs`, `crates/flamewm-platform/src/settings.rs`, `crates/flamewm-settings/src/lib.rs`, `ui/settings.html`: shorter, linear opt-in maximize; disabled by default; rename maximize Magic Lamp to **Scale** while preserving true Magic Lamp in minimize; legacy saved maximize `magic-lamp` migrates to `scale`. Unpainted-client capture guard.
- `tools/test_flamewm_graphical_flicker_pass16.py`: six source-level contract tests covering these seams. Native Rust tests added to owning modules for shape coalescing, resize effect order, capture gate, and linear progress.

## Known limitations and required native runtime gates

This source export does not contain all build-time assets present in the user's live repository, and there is no Cargo/rustc toolchain in the authoring container. **No real native build, 1:1 X11 animation or transparency proof is claimed**. The new XShape work addresses silhouette transparency; with no compositing manager, *semi-transparent pixels inside* the silhouette cannot sample a live root backdrop using a one-time root image. The window-open effect still arms on an Expose *after the initial map*; a first-map flash may require coordinated map/redirect/present changes at the WM and XComposite platform boundary. Skipping animation for a flat client capture is a visual-quality fallback, not a guarantee of complete client readiness for all applications.

Run on the actual repository after integrating the patch:

```
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-reactor -p flamewm-render-x11 -p flamewm-wm-x11 -p flamewm-platform -p flamewm-settings
CARGO_BUILD_JOBS=2 cargo build --release -p flamewm-wm -p flamewm-desktop -p flamewm-shell -p flamewm-settings
./xephyr
```

Native acceptance must include (a) Start/Battery/Calendar close in front of a moving/dynamic background with rounded transparent corners displaying *current* pixels, (b) no unmasked frame above taskbar at close, (c) window opens with Scale/Fade on app with asynchronously painted body, (d) resize and maximize preserving visible titlebar icons, (e) no black/square lamp/fade corners, (f) maximize animation default disabled on clean config and legacy migration correctness. Record X11 logs, screenshots and exact commands. A failing native gate supersedes these source guards; write a discriminating regression for the first failing mechanism rather than adding more local masking patches.

## Offline validation

Focused Python suites: source contracts (6), previous slide contracts (8), workspace edge (7), Debian package (27), source archive (4), desktop conversion contracts (2), flicker (10), lamp (20), frame interval (12), XTest (53), CSS (2), Cargo desktop conversion gate (14), UI guard tests (8): **173 successful test cases** total. `python3 -I tools/ui_contract_guard.py`: **0 errors, 24 pre-existing warnings**. No native Rust tests or visual acceptance tests were run. Broader Python sweep timed out due to unrelated environment/integration dependencies; use bounded suites for repair qualification.

## Merge instructions

The standalone Pass 15→16 patch is intended for the live repository. Run `git apply --check <patch>` before applying, and reconcile conflicts with concurrent agent changes; do not replace the full live Git tree with this source export. Source archive modified-file mtimes are updated to avoid stale Cargo fingerprinting from the earlier 1970 source-export metadata.
