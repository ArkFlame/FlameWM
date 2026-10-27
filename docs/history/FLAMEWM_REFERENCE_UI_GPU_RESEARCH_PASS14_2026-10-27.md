# FlameWM Pass 14 — External UI/UX and GPU architecture review

**Source baseline:** FlameWM `0.1.2` Pass 13 (`FlameWM_0.1.2_Pass13_X11_API_Dependency_Fixed_27_10_26.tar.gz`).
**References:** attached `cosmic-epoch.tar.gz`, `creamui-main.zip`, `coconut-main.zip` snapshots. They were read locally; no external code was copied into FlameWM.
**Status:** two bounded implementations completed; GPU backend migration/configuration deferred pending native evidence and a visually equivalent software backend.

## Existing owners and non-regression boundaries

- FlameWM is a standalone Rust desktop, not the old IceWM C++ fork (`docs/ARCHITECTURE.md`).
- `flamewm-render-core` owns the retained document, paint commands, dirty state and scroll offsets. `flamewm-render-compiler` is the authoritative build-time CSS parser. `flamewm-render-x11` owns X11/Xft/XRender drawing. `flamewm-ui-x11` adapts the common models. Do not add a second CSS interpreter or product-side paint engine.
- X11 uses native XRender compositing for alpha and other X11 painting paths (`crates/flamewm-render-x11/src/app/mod.rs`, `app/render.rs`, `xrender.rs`). Hardware execution of XRender operations depends on the X server/driver, not the presence of the XRender extension alone. Replacing it with Vulkan would create a second GPU renderer alongside XRender.
- Wayland already renders through Smithay 0.7 `GlowRenderer` (OpenGL ES) using `WinitGraphicsBackend<GlowRenderer>` or DRM/GBM/EGL (`crates/flamewm-wm-wayland/src/state.rs`, `drm/mod.rs`); output damage tracking is already present. **GPU support is not missing.** The XWayland UI clients still use X11 rendering and buffer paths.
- There is **no proven pixel-equivalent software-compositor fallback for Wayland** in this source snapshot. Merely offering a GPU-off toggle or selecting Mesa software OpenGL would not satisfy a real low-overhead CPU backend; do not mislabel it.

## Reference findings and adoption decisions

| Reference and source evidence | Proven concept | Decision for FlameWM |
|---|---|---|
| CreamUI `crates/render/src/display_list.rs`, `backend.rs`, `gpu.rs`, `cpu/*` | A backend-independent display list can be replayed by GPU or CPU, and GPU context can be shared per process. It supports explicit backend policy and startup fallback. | **Adopt the principle, not the framework.** FlameWM already has `PaintCommand` and XRender/CPU plumbing. A future software/GPU parity renderer must consume the same `PaintCommand` stream without a new UI tree. Do not add wgpu to every shell component. |
| CreamUI `crates/core/src/runtime/{transaction.rs,mod.rs}` and `damage.rs` | Retained tree mutations damage affected bounds; damage becomes full when too fragmented or large. | **Already implemented** in FlameWM's `model/damage.rs` (dirty nodes and area threshold). Do not duplicate. Repair one source-proven wasted invalidation at clamped scroll boundaries (implemented). |
| COSMIC `cosmic-comp/src/backend/kms/render/{gles.rs,pixman.rs,mod.rs}` | Graphics API backends can be split behind compositor-owned output rendering. | **Conditional reference only.** Smithay already owns FlameWM's Wayland compositor backend. Porting COSMIC's render subsystem creates complexity and exceeds current proof. A future fallback should be introduced at the compositor render boundary, not through feature-level forks. |
| COSMIC `cosmic-comp/src/backend/kms/surface/*` and `backend/render/*` | Per-output rendering, GPU resource lifetime and frame completion are centralized. | **Retain** existing FlameWM output-damage tracking and frame scheduling; investigate dma-buf/direct scanout and frame pacing independently before adding a new API. |
| Coconut `apps/bg/src/{gpu.rs,render.rs,main.rs}` | Process-wide shared wgpu device, output-local swapchains, fallible GPU startup, CPU static-image fallback and callback-paced animated shader frames. | **Adopt the scope/failure pattern** only if FlameWM later needs GPU-only custom effects; never keep a GPU device alive only for a static desktop. Its static fallback is not pixel-equivalent to animated shader effects, so it is not suitable as a transparent compositor toggle. |
| Coconut `crates/core` and `integrations/*` | Control surfaces and integrations separated by capability; platform-specific behavior remains behind adapters. | **Already architecturally aligned** with FlameWM Platform/Control/UI. Do not import Coconut's plugin registry or duplicate integration services. |

The three projects do not provide a drop-in fix for FlameWM's present X11 render path. Their libraries, UI models and packaging must not become FlameWM runtime dependencies merely as a styling shortcut.

## Implemented in this pass

1. **CSS reset support** in `crates/flamewm-render-compiler/src/compile.rs`: uniform border shorthand now accepts `none`, `0`, `0px` in addition to the existing `<px> solid <color>`. Reset sets width and color without adding drawing primitives or changing compiled document format. The quick Task Manager CSS preflight matches these accepted forms, with native compiler tests added. Invalid multi-edge/complex borders remain rejected instead of being silently misrendered.
2. **No-op scroll invalidation** in `crates/flamewm-render-core/src/model/interaction.rs`: once the clamped offset equals the current offset, no document revision, document damage or new offset-map entry occurs. Actual scroll and clamp-on-content-shrink still invalidate. `crates/flamewm-ui-x11/src/surface_runtime.rs` likewise skips redundant retained-offset map inserts. Rust regression tests cover boundary spam, actual scrolling, further overscroll, and content shrink.

Impact: fewer redundant document invalidations and allocations during trackpad/wheel activity at scroll boundaries. The X11 event dispatcher still marks wheel-input repaint for application callbacks; this pass does not claim to eliminate all boundary-scroll paint work. No incremental idle process/PSS benefit is claimed. CSS parsing is build-time and has no runtime memory tax.

## GPU policy and planned acceptance gates

**Current recommendation: preserve hardware GLES for Wayland and server-native XRender for X11. No new Vulkan backend.** Vulkan/wgpu would require another renderer, texture/resource lifetime, shader and glyph parity, fallback semantics and significant GPU memory; the current code has no measured evidence that this would save total CPU+RAM.

A future user-configurable `Rendering: Auto / GPU / Software` setting becomes valid only **after** a genuine feature-parity software compositor is available. At that point:

1. One model (`PaintCommand` for Flame-owned UI, Smithay compositor render elements for native Wayland surfaces). No feature-level conditional rendering branches.
2. One compositor/backend capability selection at startup (not a shader fork per window). Explicitly report selected backend and reason, and fall back only when results are equivalent. A software mode must not silently disable alpha/blur/text/cursor mechanics.
3. The GPU path must share context/resources per process and use damage regions; no per-window wgpu devices. GPU memory accounting and CPU thread/wakeup accounting are required.
4. Measure both **process PSS and Xorg/XWayland server PSS**, GPU VRAM, CPU% per process, idle wakeups, frame latency, pixmap memory and power on old Intel/AMD/NVIDIA and software Mesa. A GPU acceleration toggle might reduce CPU while increasing PSS/VRAM, so track both.
5. Verify text rendering, app alpha, popup corners, blur, shadows, cursor, selection, window movement/resize, fullscreen, multi-monitor, fractional scale and nested Xephyr; no visual drift accepted without explicit product authorization.
6. Keep the installed renderer default unchanged until those gates pass. Do not claim a 'disable GPU' option when it just sets `LIBGL_ALWAYS_SOFTWARE`, which often increases CPU and merely changes the OpenGL driver.

Useful local observations (run on the actual desktop, not in a headless source packager): `glxinfo -B` for X11 renderer strings, `eglinfo` for EGL capabilities if installed, `ps` and `/proc/*/smaps_rollup` for PSS, `tools/bench-flamewm-session.py` for nested CPU/PSS, and compositor profiling from `FLAMEWM_PROFILE`. Record GPU/driver and session type with every comparison.

## Verification performed in source-packaging environment

- Existing Python task-manager/CSS, shared-surface cache/dependency, desktop convert and task-manager integration checks: 20 passed.
- New Rust unit tests added, **not executed** (the Rust toolchain and live graphical compositor are unavailable here).
- No external source, font assets or executable binaries copied. No added crate dependency, config migration, WM behavior alteration or new process.
- Other historical specs calling FlameWM an IceWM C++ fork are superseded by the active Rust architecture (`docs/ARCHITECTURE.md`).
