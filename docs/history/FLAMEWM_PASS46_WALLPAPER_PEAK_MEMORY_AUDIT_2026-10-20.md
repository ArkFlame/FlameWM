# FlameWM 0.1.0 — Pass 46: wallpaper peak-allocation reduction

**Baseline:** the provided Pass 45 integrated source overlay. **Delivery:** a newly named replacement source overlay (not a standalone binary or complete source release: assets/ and historical `.vendor/` materials were not supplied). Pass 45's screenshot X11 row batching, in-place crop, icon-cache budget guard, and Pass 41–44 compilation fixes are preserved.

## Source-proven allocation problems and corrections

1. **Cropped `Fill` wallpaper previously generated an entire oversize intermediate.** `crates/flamewm-image-core/src/wallpaper.rs::compose` computed a scaled `RgbaImage` of `(sw, sh)` even though part would be cropped, then allocated another target canvas and blitted. The existing `wallpaper::compose_region` had already been independently checked pixel-for-pixel against a pre-region `reference_compose` across all five fit styles, aspect ratios and cropped regions. Pass 46 uses that existing compositor only if `Fill` requires resampling to a layer larger than the final target; it writes only visible pixels to one target-sized RGBA image. It **does not** change normal direct full-coverage scale, borrowed natural-size image composition, Fit, Center, or Tile paths. Added another explicit 4-channel, both-overdraw-axis differential Rust test.
2. **Solid/letterbox canvas used an iterator without an exact lower size hint.** `solid` constructed the RGBA frame with `colour.iter().copied().cycle().take(count).collect()`, potentially growing the backing `Vec` by repeated reallocations with spare capacity. Pass 46 allocates the exact requested logical frame length once (`vec![0u8; count]`) and fills four channels per pixel. It retains checked dimension multiplication and the exact former RGBA output. Added a Rust unit test for odd-size fills and alpha preservation.

### Peak-memory arithmetic (not RSS measurements)

For a natural **3840×2160** RGBA wallpaper rendered with cropped `Fill` into a **1920×1200** target, the old path allocated a `2134×1200×4` (~**9.77 MiB**) scaled staging image **in addition** to the `1920×1200×4` (~**8.79 MiB**) final canvas. Pass 46 no longer allocates that ~9.77 MiB staging image for this specific path. For a 1080×1920 portrait target, the avoided staging image is about **25.00 MiB** (3414×1920×4). These are deterministic logical allocation sizes, **not** whole-process PSS/RSS deltas, and do not count allocator metadata, decoded natural image, X11 buffers, or the final retained canvas. A `Fill` that already covers the target exactly does not change. Solid canvas allocation now uses an explicit initial-size reservation; saved spare capacity depends on the prior allocator's growth behavior.

## Regression and behavior boundaries

- `wallpaper::region_tests::compose_region_matches_cropped_compose_for_every_fit_root_rect_and_natural` remains the existing pixel oracle across `Fill`, `Fit`, `Stretch`, `Center`, `Tile`, including negative/out-of-bounds crops and varying RGBA inputs.
- New `cropped_fill_uses_only_the_visible_canvas_and_matches_reference` checks both aspect-ratio overdraw orientations with irregular source/target dimensions against the previous implementation.
- New `solid_canvas_reserves_once_and_preserves_all_channels` checks buffer length and byte-for-byte fill, without asserting allocator-specific spare capacity.
- New Python `tools/test_flamewm_pass46_wallpaper_memory_contract.py` source guards: **two tests fail against Pass 45 and all four pass against Pass 46**. They are source-shape guards, not substitutes for Rust unit tests.
- Selected existing source/packaging Python suites and UI guard pass. Pass 45 and Pass 46 architecture guards have the **same seven pre-existing violations**: applications X11 dependency and symbol, applications env, desktop colorkey, screenshot env, WM session env, and desktop CSS display:none.

## What was deliberately not changed

- Resident 8-MiB task/shell icon raster budgets and render backbuffers: blind shrinking would trade RAM for slower icon loads and visual flicker.
- Wallpaper's retained root raster in the document and its loader: refcount conversions would cross retained UI ownership and invalidate several drawing assumptions.
- Wallpaper per-output scratch raster: a direct compositing implementation would be a larger architectural change with unverified performance, not a safe one-line reduction.
- Long-running desktop service process or cross-monitor layout/lockscreen/SDDM behavior: not memory regressions proven by this audit.

## Native acceptance on your complete checkout

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS46_WALLPAPER_PEAK_MEMORY_REDUCTION_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
tar --touch -xzf "$ARCHIVE" -C . && \
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-image-core -p flamewm-desktop && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot -p flamewm-integrations-linux && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

In a graphical test session compare before/after repeated per-monitor and shared wallpaper switches for `Fill`, `Fit`, `Tile` and portrait/landscape mixed monitors; collect `./scripts/bench-memory` PSS/RSS at matched screen sizes and workloads. The Rust tests, actual peak-memory counters, release build, and native X11 integration **were not run here** (Rust toolchain and full runtime assets absent).

**Skipped:** speculative renderer or cache budget changes, full-native release and graphical profiling. **Risk:** the reused region compositor has established source-level pixel oracle tests but the new call site has not been exercised by a native Rust build; native build and pixel regression tests are required before production deployment.
