# FlameWM 0.1.3 — Pass 37: shared window chrome and GPU status

## Baseline

Complete, root-relative Pass 36 archive, not the historical IceWM fork. All source edits in this pass are limited to the X11/Wayland title-control visual path, first-party control assets, a broken archive allowlist, focused regression coverage, and this note. No new Cargo dependencies. The workspace remains on Smithay 0.7 and the existing Rust 2024 build.

## Functional change

1. `crates/flamewm-skin/src/recipes/window_chrome.rs` now owns `control_glyph_rgba(WindowControlRole, edge, rgb)`, a small antialiased, dependency-free vector-to-RGBA fallback for minimize, maximize, restore and close. Bounds are 4..=128 pixels and callers receive straight-alpha RGBA; no GPU/native handle leaks into the shared interface.
2. `crates/flamewm-wm-x11/src/chrome.rs` uses this shared fallback if its packaged semantic control SVG cannot be read or decoded. It preserves the existing bounded raster cache, per-button material colors, and X11 hitbox/geometry ownership.
3. `crates/flamewm-wm-wayland/src/render.rs` no longer contains a competing hand-coded glyph-line engine. It uses the same shared glyph coverage, shared control geometry, the correct restore artwork while maximized, and a circular red close-hover backplate instead of a rectangular solid block. The compositor keeps its existing GPU-backed Smithay `MemoryRenderBuffer` upload/caching.
4. `assets/web/breeze/window-{minimize,maximize,restore,close}.svg` contains original FlameWM artwork under the source paths that X11 already requests. These are not copied KDE Breeze icons and use the existing `ColorScheme-Text` scheme. The SVGs are tiny, have no remote resource references, and avoid a new runtime icon-theme dependency.
5. `packaging/source-manifest.txt`: removed stale `@exec` assertions for absent SVG brand resources; an SVG is not executable. Other missing brand resources are **not** silently claimed restored. The new `assets/` subtree is part of the complete archive.

### Preserved consumers and invariants

- X11 task/window control actions and native input-child hitboxes are unchanged.
- Wayland window model, titlebar hit-testing, placement, maximize/restore actions, and Smithay compositing are unchanged.
- The PulseAudio-compatible `libpulse` client (working with PipeWire-Pulse and PulseAudio) is unchanged; no direct PipeWire dependency was added.
- No new renderer crate, event-loop owner, background daemon, SVG decoder, toolkit dependency, or persistent GPU copy is introduced.
- Built-in Wayland title text remains 5x7 and does **not** have exact IBM Plex Sans/Xft typography parity. This is still a visual gap.

## Graphics status — do not overclaim

| Requirement | Status |
| --- | --- |
| Interchangeable provider negotiation | Existing X11 `auto`/`xrender`/`core`, Wayland `auto`/`gles` |
| Wayland EGL/GLES composition | Existing Smithay path retained; no hardware verification |
| X11 XRender | Existing server-side rendering retained; may run on CPU depending on Xorg/driver |
| Dedicated X11 hardware GL/Vulkan renderer | **Not implemented** |
| Wayland DMA-BUF zero-copy client protocol | **Not implemented** |
| Wayland titlebar glyph sharing | **Implemented in this pass** |
| X11 and Wayland exact font/blur visual parity | **Not implemented** |

Smithay source was inspected as reference only. Do not import an alternative compositor or add a duplicate hardware renderer without an end-to-end consumer, capability checks, X11 lifetime handling, and the ability to fall back safely. Native GPU measurements and driver identification remain a required target-machine gate.

## Verification

- Cargo manifests: parse/structural check (45); **not** a Cargo build.
- `tools/test_pass37_shared_chrome.py`: 4 source/asset tests.
- Previous source-contract, archive, and session tests run separately; report exact counts from the executed command.
- `scripts/package-source-archive --check` and extraction probe: validates file list/permission/paths; **not** native visual evidence.
- Native `cargo check`, `cargo test`, Xephyr/Wayland render screenshots, PSS and GPU utilization: **NOT CHECKED** in this environment (no Rust toolchain, no real X11/Wayland GPU session).

### Target Linux verification

```bash
python3 -I tools/verify_cargo_manifests.py
cargo check --locked -p flamewm-skin -p flamewm-wm-x11 -p flamewm-wm-wayland
cargo test --locked -p flamewm-skin -p flamewm-wm-x11 -p flamewm-wm-wayland
./scripts/xephyr
./scripts/wayland
```

On a machine with real graphics, compare title buttons and hover backgrounds in both nested sessions, including maximized/normal state and narrow titles; inspect `glxinfo -B` and the compositor's startup diagnostics. **An XRender extension does not by itself prove GPU execution.**

## Known risks

- The new SVGs are first-party replacements, not exact copies of the missing approved Breeze files. Their visual match requires screenshots and normal SVG decoder tests.
- `assets/web/github.svg` and `assets/web/paypal.svg` were already absent in Pass 36; the installation acceptance contract still names these paths. This pass does not fabricate brand icon assets.
- The shared raster's glyph appearance and the Wayland pixel blending have Rust unit tests but were not compiled or executed here.
