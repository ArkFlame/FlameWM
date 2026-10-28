# FlameWM 0.1.2 — Performance Pass 9: shared surfaces and X Shape streaming

2026-10-27. Input: `FlameWM_0.1.2_Pass8_Glass_Surfaces_Explorer_Performance_27_10_26.tar.gz`. Scope: source compatibility preserved; no settings key/default, UI geometry, painting order, window focus, input event, or icon asset changes.

## Source findings and plan

1. Canonical color policy was stored in `flamewm-ui-core::context_menu` even though Shell, Explorer, Settings, Terminal and the native WM consume it. Four consumers separately unpacked the same `SurfaceFill.rgb` and `.alpha` into an X11 renderer color; frame border and software titlebar painted the same alpha blend through separate code. Move settings interpretation and pixel blending to `flamewm-ui-core::surface`; provide one X11 adapter in `flamewm-ui-x11::surface_style`, retaining old module re-exports.
2. `flamewm-render-x11::xshape::apply_clipped_mask` did `clip_spans()` (heap `Vec`) followed by `spans_to_rects()` (another heap `Vec`). Stream the clipped spans directly to one rectangle builder. Leave raw shape handling and merge geometry unchanged, and retain an owned test-only helper for regression comparisons.
3. Do not add polling, new native windows, a theme manager, or another state authority. Shared surface configuration keys remain `surfaceColor`, `surfaceOpacity`, `surfaceBorderColor` and `surfaceBorderOpacity` (white/25% default contour); dev-only Explorer opacity overrides and per-app fill behavior remain intact.

## Implemented owners and consumers

- `crates/flamewm-ui-core/src/surface.rs`: single `SurfaceFill`, `surface_fill`, `surface_stroke`, `blend_surface_over_rgb` owner. `context_menu` re-exports the prior public paths.
- `crates/flamewm-ui-x11/src/surface_style.rs`: `From<SurfaceFill> for UiColor`, `surface_background`, `surface_border`, `project_surface_border`.
- `crates/flamewm-shell/src/settings_projection.rs`: same native color adapter for shell panel and popups, with panel-specific transparent background mode preserved.
- `crates/flamewm-explorer/src/theme.rs`: common stroke conversion; Explorer's fill and backdrop still use the existing live/override opacity behavior.
- `crates/flamewm-terminal/src/app.rs`: common stroke conversion; terminal theme and cache semantics unchanged.
- `crates/flamewm-settings/src/lib.rs`: common border projection.
- `crates/flamewm-wm-x11/src/chrome.rs`, `src/frame/chrome.rs`: common exact integer RGB alpha-blend. The RGBA titlebar's original alpha channel remains unchanged.
- `crates/flamewm-render-x11/src/xshape.rs`: clipped span iterator consumed by the existing rectangle-coalescing code; unclipped masks also use that same iterator-accepting function.

## Performance expectation

- For each clipped rounded mask projection: eliminate **one transient heap vector** (previously `Vec<(u32,u32,u32)>` with up to one tuple per visible row). A full-height 1080-row intermediate payload would be ~12.7 KiB before capacity overhead, but clipping normally produces fewer rows. No change to resulting rectangle count or X Shape protocol transfer volume.
- Reusing one surface-color conversion and one pixel blend is principally a maintenance/determinism improvement, **not** evidence of meaningful CPU or memory savings. No additional background threads, timers, resident buffers or X requests were added. Expect negligible overall idle CPU/PSS change; any animated popup improvement depends on actual number of clipped-mask updates.
- Preserve prior passes 1–8 unchanged except the explicitly listed shared consumers.

## Verification executed

- `python3 -m unittest discover -s tools -p test_flamewm_explorer_source_contract.py -q`: 6 pass.
- `python3 -m unittest discover -s tools -p test_flamewm_settings_acceptance_schema.py -q`: 155 pass.
- `python3 -m unittest discover -s tools -p test_flamewm_context_menu_weather_acceptance.py -q`: 14 pass.
- `python3 -m unittest discover -s tools -p test_flamewm_pass26_corner_shadow_compile.py -q`: 4 pass.
- `python3 -m unittest discover -s tools -p test_flamewm_pass52_settings_css_contract.py -q`: 2 pass.
- A separate `test_flamewm_surface_icon_slide_regressions.py`: 5 pass / 3 fail. Re-run against untouched Pass 8 gives **the same three failures**, concerning outdated icon picker and panel-backdrop source assertions. Tests not rewritten.
- Independent deterministic Python checks: 50,000 RGB alpha blends and 1,200 old-owned vs streamed-clipped shape constructions; equality confirmed. Added Rust unit tests for native color projection, exact blend parity and shape-mask streaming.

## Remaining verification gates

No Rust toolchain available in this environment. On a workstation, run:

```bash
cargo fmt --all -- --check
cargo test -p flamewm-ui-core -p flamewm-ui-x11 -p flamewm-render-x11 -p flamewm-wm-x11 -p flamewm-explorer -p flamewm-terminal -p flamewm-shell -p flamewm-settings
cargo build --release
```

Check live X11: 0%, 25%, 100% white/colored contour on terminal/explorer/settings/Start/taskbar/weather/Wi-Fi/audio/calendar popovers; resized native titlebar; popup slides with rounded/clipped masks; monitor scaling; unchanged shadow and drag behavior. Measure PSS and CPU against Pass 8 before reporting numerical performance wins.

**Not checked:** Rust compilation, live X11 visuals, Wayland titlebar parity, actual CPU/PSS savings. Risks: any new type-level compiler error is undetected until native build; the prior three unrelated Python failures persist.
