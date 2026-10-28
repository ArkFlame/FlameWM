# FlameWM 0.1.3 — Pass 34 source fixes and remaining graphics milestones

## Source and status

Baseline: complete Pass 33 source archive, based on the original October 28 source upload. Pass 34 is a **complete, standalone source tree**, not an overlay. The current implementation is the independent Rust workspace; historical IceWM/C++ materials and the supplied upstream Smithay repository are reference-only.

Estimated implementation progress against the user's *original functional objectives*: **approximately 55%**. This is a coarse engineering estimate, not a completion/test percentage. All release packaging and manifest regression checks being green does **not** make GPU delivery green.

| Requirement | Status | Evidence / limitation |
|---|---|---|
| Remove/replace unnecessary dependencies | Partial | libpulse isolated behind opt-in `pulse-audio`; DBus/Chrono consolidated (Pass 33). Source use audit found no proven unused direct external dependencies safe to remove. No major Smithay/calloop/X11 replacement justified. |
| PipeWire/PulseAudio policy | Implemented | Keep `libpulse-binding` for native asynchronous PulseAudio-protocol audio control; works with PipeWire-Pulse or PulseAudio. Native PipeWire-only hosts without `pipewire-pulse` need another adapter (not included). |
| Wayland titlebar visual bugs and X11 commonality | Partial | New shared skin-owned title elision and bounds calculation, used by both live X11 frame painting and Wayland GLES-memory title buffer, with no button overpaint for long titles. Both already use shared chrome metrics/palette/control slots. Actual font parity remains missing: Wayland uses its small bitmap glyph fallback. Requires visual inspection on real Wayland. |
| GPU acceleration on X11 | Partial | Existing XRender/Xft operations execute on the X server and can run on GPU when accelerated by the host Xorg/XWayland driver. Exposed explicit XRender vs core-alpha backend selection in the shell and WM decoration consumers. No new GLX/EGL/OpenGL scene renderer implemented and no hardware execution verified. |
| GPU acceleration on Wayland | Existing GPU pipeline retained; not hardware-proven | Smithay GlowRenderer renders the compositor through EGL/OpenGL ES in nested winit and optional native DRM/GBM paths. Mesa llvmpipe may still be software. No separate CPU rasterizer introduced. |
| Interchangeable graphics providers | Partial | Engine-neutral `flamewm-api::graphics::{GraphicsProvider,DisplayProtocol,choose_provider}` used by both X11 renderer entrypoints and the Wayland compositor. Runtime X11 options: `auto`, `xrender`, `core`; Wayland: `auto`, `gles`. A second Wayland GPU provider or an X11 direct EGL/GLX provider is **not** implemented. |
| User-reported Cargo manifest failure | Fixed, source-checked | `rustix` moved from `[features]` to `[dependencies]` in `flamewm-integrations-linux`. A semantic manifest validator runs before `./scripts/wayland` builds and source-archive generation. 45 manifests parse and feature entries are arrays, not maps. |
| Missing Pass 29 archive filename | Usage corrected | The old relative path does not exist in the user's checkout. Pass 34 is a downloadable, complete source archive and requires extracting the **actual downloaded filename** with `--strip-components=1`. No source change can make an absent Pass 29 file exist. |

## Changed owners

- `crates/flamewm-integrations-linux/Cargo.toml`: correct the moved rustix dependency.
- `crates/flamewm-skin/src/recipes/window_chrome.rs`: authoritative layout bounds and text truncation; pure regression tests.
- `crates/flamewm-wm-x11/src/chrome.rs` and `src/frame/chrome.rs`: apply shared title elision *before* text measurement, centering, and Xft drawing.
- `crates/flamewm-wm-wayland/src/render.rs`: keep the generated bitmap title out of the control region.
- `crates/flamewm-api/src/graphics.rs`, `src/lib.rs`: cross-protocol provider negotiation, typed values and tests.
- `crates/flamewm-render-x11/Cargo.toml`, `src/app/mod.rs`, `src/external_drawable.rs`: select the same provider for X11 shell and external WM chrome renderers; explicitly log fallback. A forced `core` selection chooses a non-ARGB window visual and OpaqueFallback so translucency cannot silently fail after paint.
- `crates/flamewm-wm-wayland/src/main.rs`: reject unsupported Wayland provider requests before opening devices or starting the renderer; retain proven GLES rendering.
- `tools/verify_cargo_manifests.py`, `tools/test_pass34_graphics_and_manifest.py`, `tools/test_package_source_archive.py`: semantic manifest and source-wiring regressions.
- `scripts/wayland`, `scripts/package-source-archive`: validate manifests before builds/releases rather than packaging a non-buildable workspace.

## Functional checks actually run in the packaging container

- `python3 -I tools/verify_cargo_manifests.py` — 45 manifests pass.
- Pass 33 dependency/chrome source contracts — 6 pass.
- Pass 33 merge contracts — 7 pass.
- Source archive tests — 18 pass.
- Wayland wrapper tests — 14 pass.
- Pass 34 additional source contracts — 5 pass.
- `bash -n scripts/wayland scripts/flamewm-wayland-session scripts/apply-pass30-cargo-layout` — pass.
- First-party manifest dependency graph — 44 crates, no cycles.

**Unavailable in this container:** Rust/Cargo toolchain, network to install it, GPU/DRM/X11 graphical session. No claim that `cargo check`, Rust tests, or GPU visual tests passed. This release must not be described as fully functioning GPU-accelerated on both protocols without target-machine evidence.

## Build and runtime verification required on the target system

```bash
python3 -I tools/verify_cargo_manifests.py
cargo metadata --locked --no-deps --format-version 1 >/dev/null
cargo check --locked -p flamewm-api -p flamewm-skin -p flamewm-render-x11 -p flamewm-wm-x11
cargo check --locked -p flamewm-wm-wayland
cargo check --locked -p flamewm-wm-wayland --features drm
cargo test --locked -p flamewm-api -p flamewm-skin -p flamewm-render-x11 -p flamewm-wm-wayland
./scripts/wayland
```

When investigating hardware execution, collect `glxinfo -B` and `eglinfo -B` (where available), the Xorg/XWayland `glamor`/Mesa renderer logs, and CPU/VRAM/PSS measurements of the compositor *and X server*. A successful OpenGL ES context or XRender library load alone does not prove GPU use.

## Correct archive extraction (do not overwrite local changes blindly)

After downloading the exact Pass 34 archive into `~/Downloads`:

```bash
mkdir -p "$HOME/Downloads/flamewm-pass34"
tar -xzf "$HOME/Downloads/FlameWM_0.1.3_Pass34_Manifest_Titlebar_Graphics_Complete_Source_20261028.tar.gz" \
  -C "$HOME/Downloads/flamewm-pass34" --strip-components=1
cd "$HOME/Downloads/flamewm-pass34"
python3 -I tools/verify_cargo_manifests.py
./scripts/wayland
```

Use `diff -ru` or version control to merge into an existing dirty checkout. Avoid extracting the complete archive over ongoing work without reviewing conflicts.
