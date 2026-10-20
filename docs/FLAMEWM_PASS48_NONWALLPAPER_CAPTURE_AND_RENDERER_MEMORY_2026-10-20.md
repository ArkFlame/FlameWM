# FlameWM Pass 48 — Non-wallpaper memory audit and source repair

**Baseline:** Pass 47 integrated source overlay, FlameWM workspace 0.1.0. **Scope:** X11 screenshot readback and renderer image staging only. Wallpaper composition, desktop layout, WM window state, icon-cache budgets, and animation semantics are unchanged.

## Audited memory owners, ordered by possible per-operation exposure

| Owner | Source-supported exposure | Disposition |
| --- | --- | --- |
| X11 window-animation captured images, `flamewm-wm-x11::animation::CaptureCache` | Bounded to 64 MiB / 8 minimized windows, for legitimate restore animations | Keep; reducing the cap without multi-window restoration benchmarks would change effects |
| X11 renderer per-surface image pixmap cache, `flamewm-render-x11::app::images` | 16 MiB / 64 entries **per X11App**; retained server resources | Keep bounds; remove one avoidable allocation in its XRender fallback |
| On-demand screenshot GetImage, `flamewm-screenshot::capture` | A 3840×2160 reply has 33,177,600 bytes (31.64 MiB) before RGBA conversion; the old decoder allocates another full RGBA frame | Fixed for tightly packed 32-bpp replies; legacy decoder still handles other formats |
| Shell icon raster cache, `flamewm-shell::async_projection` | 8 MiB nominal byte budget; entry cap 2048; output clones may cost more temporarily | Left unchanged, as the cache stores real UI rasters and broad ownership changes require native proof |
| WM titlebar backdrop cache | 4 MiB retained titlebar backdrops plus drag snapshots with explicit teardown | Unchanged: active drag screenshot and blur are behavior-sensitive |

Budget values above are code constants, **not measured live process RSS/PSS or total GPU/X-server memory**.

## Implemented

1. `crates/flamewm-screenshot/src/capture.rs`: the common packed 32-bpp X11 reply is converted to RGBA in its **existing owned Vec**. Tests verify byte-order/mask fidelity and allocation identity. Replies with padding, non-32-bpp depths or mismatched sizes go through the unchanged general decoder. `grab` passes `reply.data` by value instead of borrowing it.
2. `crates/flamewm-screenshot/src/ui.rs`: annotated-editor X11 readback uses the same owned decoder, so the change covers both ordinary captures and editor exports.
3. `crates/flamewm-render-x11/src/app/images.rs`: premultiplied ARGB staging for a cache miss is created only inside the verified XRender+32-bit-visual path, after successful pixmap creation. The opaque pixmap and alpha-mask fallback no longer creates a temporary ARGB buffer it never uses. Image colors, cache eviction, and fallback paths remain unchanged.
4. `tools/test_flamewm_pass48_nonwallpaper_memory.py`: four focused checks cover both owned-image call sites, 32-bpp/fallback guarding, deferred renderer staging, and preservation of existing cache bounds.

## Expected allocation reduction

At 3840×2160 on a tightly packed 32-bpp X11 server, a screenshot capture/editor readback avoids **one additional 33,177,600-byte (31.64 MiB) heap allocation**. It still owns the X11 reply and the eventual pixel buffer (one allocation reused). For a renderer asset of W×H, the fallback avoids a previously unused `W×H×4` byte ARGB staging vector; size depends on UI content and no claim is made that this improves the normal XRender route. Benefits are during capture and image-cache misses, **not necessarily idle process RAM**.

## Evidence

- New source checks intentionally fail against Pass 47 and pass against Pass 48.
- Existing screenshot integration, Pass 45/46/47 memory contracts, Xephyr bootstrap source contract, packaging-pipeline and source-archive tests: **44/44 Python checks passed** in current working tree.
- UI contract guard: zero errors; 24 existing warnings.
- Architecture guard: seven violations already present on Pass 47, unchanged by this pass.
- Archive must be extracted over a complete source checkout with runtime assets; this snapshot is an integrated source **overlay**, not the full asset-bearing standalone release.
- Cargo/rustc and a graphical X11 server are unavailable in this authoring environment. **Native compilation, protocol readback, screenshot output, renderer fallback, and PSS/RSS performance comparison remain UNVERIFIED.** Do not treat static tests as runtime proof.

## Required native verification

```bash
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 cargo test -p flamewm-screenshot -p flamewm-render-x11
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

For memory measurements, compare Pass 47 and Pass 48 on the same X11 display/resolution using `time -v flamewm-screenshot --full --output /tmp/<unique-file>.png`, with a comparable output path and separate binaries/target directories. Compare Maximum resident set size, pixel correctness, and invocation latency; account for X-server memory separately. Do not run a live WM replacement or logout from a productive session without explicit permission.

**Skipped:** wallpaper changes, cache-budget reduction, broad UI state rewrites and unsafe direct XImage borrowing. **Risk:** source-level tests pass but Rust or X11 platform incompatibilities may still be exposed by native verification.
