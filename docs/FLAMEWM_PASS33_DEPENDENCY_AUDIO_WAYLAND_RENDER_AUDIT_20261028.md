# FlameWM 0.1.3 — Pass 33: source repairs, dependency gates, render-path audit

**Source authority:** full upload `1207_28_10_26.tar.gz`, October 28, 2026. This package is a **complete source tree** rather than a patch to an earlier archive. Current production architecture is independent Rust, not the historical IceWM C++ fork.

## Decisions

1. **Keep the PulseAudio client protocol.** PipeWire's `pipewire-pulse` is a PulseAudio-compatible server whose clients normally use `libpulse`; a second native PipeWire adapter would add a new implementation with no demonstrated memory or latency benefit. Native PipeWire without `pipewire-pulse` is **not** supported by this adapter. See https://docs.pipewire.org/page_module_protocol_pulse.html .
2. **Keep Smithay 0.7 for the native Wayland compositor.** It already owns the nested winit EGL/GLES renderer, native DRM/GBM EGL/GLES path, output-damage lifecycle, input and Wayland protocol surfaces. Replacing this stack with a partial home-grown compositor would lose mature protocol and device correctness. `backend_drm` belongs only to the `drm` Cargo feature. `backend_gbm` implies DRM through Smithay's feature graph.
3. **Keep XRender/Xft as the X11 presentation path.** It is server-composited and *may* be GPU accelerated by the host X server and driver, but cannot guarantee hardware acceleration. It has core X11 copy fallback for opaque images, and rejects alpha when the server cannot provide it. Implementing GLX, EGL or wgpu now would duplicate the existing `PaintCommand` consumer and would require a full pixel-parity, memory and synchronization proof first. No claim of a new X11 GPU renderer is made.
4. **Share the existing window-chrome recipe.** `flamewm-skin::recipes::window_chrome` already defines exact titlebar/button geometry, styling and text placement. Both X11 and Wayland now use that recipe for chrome geometry. The client protocol and final rasterizer remain backend-owned.

## Changes made

- `crates/flamewm-wm-wayland/src/render.rs`: adapt Smithay 0.7 `MemoryRenderBufferRenderElement::from_buffer` `Result` to `Option`; use skin-owned button slot geometry and `center_title_x`; correct the restore glyph for maximized windows; invalidate the cached ARGB buffer when that state changes; reject impossible texture dimensions; avoid redundant hit tests per visible window; remove unused imports. Existing premultiplied ARGB composition and GLES rendering retained. Wayland text remains the small built-in bitmap fallback, not IBM Plex Sans/Xft parity.
- `crates/flamewm-wm-wayland/src/xwayland.rs`: explicitly pass `0_u32` for X11 `Timestamp` (fixes the reported Cargo `E0277`).
- `crates/flamewm-render-x11/src/external_drawable.rs`: fix the core X11 fallback copying transparent ARGB pixels into a drawable **before** reporting failure. Unsupported alpha and semantic blits now fail *before* allocating an image pixmap or touching the destination; opaque copies still work. XRender's alpha path is unchanged.
- `crates/flamewm-integrations-linux`: `libpulse-binding` is now optional as the `pulse-audio` feature, whose default is off. Only `flamewm-wm-x11` explicitly enables it. The Pulse stress integration test and source module are gated with the same feature. Desktop, Settings, Explorer and Shell no longer request that dependency directly.
- `Cargo.toml` and dependent manifests: consolidate existing exact `dbus` and `chrono` constraints into workspace dependencies (no versions changed). Existing `Cargo.lock` is retained; no new third-party dependency was introduced.
- `crates/flamewm-wm-wayland/Cargo.toml`: move `smithay/backend_drm` out of the nested/default feature set into `drm`. This removes the direct request for DRM in nested-only builds. The actual resolved graph must be confirmed by Cargo.
- Add `tools/audit-flamewm-dependencies.py`: manifest counts without Cargo, optionally run per-binary `cargo tree --locked --offline` to compare selected build graphs. Add `tools/test_pass33_dependency_chrome.py` and update the earlier XWayland timestamp source-contract check.

## Evidence and limitations

- Verified: 44 first-party crates; 214 direct manifest dependency declarations (167 first-party and 47 external); 335 locked package/version entries, 308 unique names across all locked platforms/features. These are *not* resident runtime dependency counts, compiled crate counts, or RAM usage measurements.
- Verified by Python source-contract regression checks: 17 passed; Wayland session-wrapper matrix: 14 passed; shell syntax checks passed.
- **UNVERIFIED:** native Rust `cargo check`, Cargo test builds, graphical Wayland titlebar parity, Xorg/XWayland accelerated execution, user GPU selection, per-process PSS/VRAM. This packaging environment has no Rust compiler/Cargo and no live desktop.
- The exact standalone Smithay source archive mentioned in the request was not present in this source snapshot; the live integration is Smithay 0.7 and its public source/API documentation was consulted. No external reference tree was copied into the runtime.

## Build and runtime gates on the target Linux machine

```bash
cargo check --locked -p flamewm-wm-wayland
cargo check --locked -p flamewm-wm-wayland --features drm
cargo check --locked -p flamewm-wm-x11
cargo check --locked -p flamewm-desktop -p flamewm-settings -p flamewm-shell -p flamewm-explorer
cargo test --locked -p flamewm-skin -p flamewm-wm-wayland
cargo test --locked -p flamewm-integrations-linux --features pulse-audio
cargo test --locked -p flamewm-render-x11
python3 tools/audit-flamewm-dependencies.py --resolve --json /tmp/flamewm-dependency-graph.json
./scripts/xephyr
./scripts/wayland
```

For actual hardware attribution, record `glxinfo -B`, `eglinfo` if available, Xorg/XWayland render driver/DRI state, and PSS and GPU memory of **both** FlameWM and the host server. On nested Wayland, Smithay's GLES may use software Mesa; GLES alone does not prove hardware execution. With no compositor, XRender is similarly driver-dependent. Validate mixed XWayland and native clients, transparency, minimize/restore, title glyphs, 1–2 monitor scale changes and fullscreen before calling the GPU deliverable complete.

**Not delivered:** a new hardware-verified X11 GLX/EGL GPU compositor, pixel-identical Wayland font rendering, native PipeWire-only control adapter, or live GPU/resource benchmarks. Those changes have not been asserted as complete.
