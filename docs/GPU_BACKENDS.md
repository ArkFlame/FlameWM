# GPU Rendering Backends: CPU, OpenGL, Vulkan

Status: design, 2026-11-08, `main` at `cb5999d1`, working tree. No source changes are made by this document. Companion: [GPU_FRAME_PAINT.md](GPU_FRAME_PAINT.md) (existing `renderAcceleration` behaviour and measurements), [QUALITY_SCORECARD.md](QUALITY_SCORECARD.md), [ARCHITECTURE_MAP.md](ARCHITECTURE_MAP.md).

## Verdict

- **Vulkan is not worth it now.** The footprint goal is violated by the GPU path already, the measured wins are narrow, and Wayland has no Vulkan renderer to switch to. Do not add `ash`, `vulkano` or `wgpu` for this.
- **OpenGL (EGL) is the only backend worth pursuing,** and only as an opt-in accelerator for whole frames. The current per-op GPU routing is not that.
- **CPU stays the default.** `auto` should not become the default until the frame-level gate in phase 3 passes.
- **Phase 1 is vocabulary and migration only, with no behaviour change.** It makes the key and the backend enum shared before any new GPU code exists.

## Current state (evidence)

| Area | What the code does today | Evidence |
|---|---|---|
| Setting | `renderAcceleration`, default `cpu`. Env override `FLAMEWM_GPU=0` forces CPU and wins; `FLAMEWM_GPU=1` forces GPU. | `crates/flamewm-render-x11/src/accel/mod.rs:164-175`; `docs/GPU_FRAME_PAINT.md` |
| Settings UI | Exposed as a text setting, not a choice control. | `crates/flamewm-settings/src/app_impl_3.rs:85`; `actions.rs:798-802` |
| X11 drawing | XRender/Xlib/Xft for drawing. CPU pixel path is `CpuPixels`. | `accel/mod.rs:17-21` |
| X11 GPU path | Blur and batched paints through EGL + GLES loaded at runtime with `dlopen` (`libEGL.so.1`, `libGLESv2.so.2`). No `glow` or GL crate on X11. Pbuffer surface, falling back to surfaceless (`EGL_MESA_platform_surfaceless`, `EGL_PLATFORM_DEVICE_EXT`). | `accel/gles.rs:110-157`; `accel/gles/loader.rs:30,80-102,187` |
| X11 fallback contract | GPU init fail, software renderer (`llvmpipe`, `softpipe`, `swrast`), any GL error, or op error: CPU for that op or process. A whole-frame failure returns to XRender before presenting. | `GPU_FRAME_PAINT.md` (Fallback rules) |
| Wayland renderer | Smithay 0.7 with `backend_egl` and `renderer_glow`; compositor uses `GlowRenderer`. GLES is the only compositor renderer found in source. | `crates/flamewm-wm-wayland/Cargo.toml`; `src/render/mod.rs:19,1157` |
| Dependency graph | No `wgpu`, `ash` or `vulkano` in `Cargo.lock`. `glow 0.16` comes from smithay. | `Cargo.lock` (grep) |
| Smithay renderers | Vendored smithay 0.7 `src/backend/renderer/`: `gles`, `glow.rs`, `pixman`, `multigpu`. No Vulkan renderer module. | `.vendor/smithay-master/src/backend/renderer/` |

### Measured (2026-11-07, NVIDIA GeForce RTX 3050 Ti Laptop, real GL, not llvmpipe)

From [GPU_FRAME_PAINT.md](GPU_FRAME_PAINT.md), medians in ms (`cargo run --release -p flamewm-render-x11 --example pixel_bench`):

| Op | CPU | GPU (before per-op fix) | Outcome |
|---|---|---|---|
| blur r12 | 34.8 | 10.8 | GPU win |
| blur r28, 800x500 | 13.4 | 6.1 | GPU win |
| fill rounded 600x300 | 3.9 | 9.7 | CPU now |
| frame batch, per-op | 85.6 | 732.7 | CPU now |
| frame batch, `paint_batch` | 87.8 | 14.7 | GPU win |

Live session, 15 s `/proc` probes, GPU on versus off:

| Process | PSS on (kB) | PSS off (kB) | CPU ticks on / off | Ctx switches on / off |
|---|---|---|---|---|
| flamewm-desktop | 168,867 | 25,670 | 0 / 0 | 0 / 3 |
| flamewm (WM) | 19,409 | 19,223 | 0 / 0 | 1 / 4 |
| flamewm-explorer | 18,754 | 18,791 | 0 / 1 | 0 / 1 |

What this says:

- **The GPU path does not meet the low-footprint goal.** Desktop PSS rises by about 143 MB. That is the EGL and driver load, not the blur. The default stays `cpu`.
- **Per-op GPU loses on small work** because each op pays for a round trip. The fallback contract keeps the frame CPU-visible, so results go back to the CPU per op. This is an inference from the documented contract, not a traced readback call.
- **Idle cost is unchanged.** CPU ticks and wakeups are equal. This is the one area where GPU is not a regression.
- **Only one GPU was tested.** Nothing here is evidence for AMD or Intel drivers.

## Vela and RustWebRender compared

| | FlameWM today | Vela (`.vendor/vela-main`, read-only) | RustWebRender 0.0.9 (`.vendor/RustWebRender-0.0.9`, read-only) |
|---|---|---|---|
| Stack | Xlib/XRender X11 WM; Smithay Wayland compositor | wlroots compositor with Qt Quick | X11 renderer crates: core, compiler, x11 |
| GPU API | GLES via EGL (X11 blur path, Wayland compositor) | **Vulkan**, mandatory: `docs/installation.md:6` says it requires a Vulkan 1.4 GPU | None found. X11 backends are Xft (text) and XRender (alpha) |
| CPU fallback | Yes on X11 (per op and process); not found on Wayland | Not found in the docs read here (unverified) | Yes, by design: `rustwebrender-x11/src/lib.rs:196-219` |
| Selection pattern | Env override, probe, process-wide fallback | Env flags: `VELA_VULKAN_VALIDATION=1`, `VELA_STATS=1` | Try preferred backend, log reason, fall back |
| GPU test tools | `pixel_bench` example | `vela-slowgpu.c` (Vulkan + GBM, simulated slow GPU), `vela-x11-present.c` (X11 Present) | None |

What to take from each:

- **Vela** is a compositor that must render every frame on the GPU, so a Vulkan-only design fits it. FlameWM is an X11-first WM that draws small windows, where per-op GPU already loses. Do not copy the mandatory-GPU requirement.
- **Vela's useful pieces** are the explicit validation and stats env flags, and a test client that simulates a slow GPU. The FlameWM equivalent is a frame-timing probe, which the gate in phase 3 needs.
- **RustWebRender** shows the selection shape to copy: prefer the best backend, log the reason in one line, fall back, never panic. FlameWM already follows this.

## Design

### Setting

- **Keep `renderAcceleration`** as the stored value for existing users. Add **`renderBackend`** with values `auto | cpu | opengl | vulkan`. The camelCase name matches the existing key.
- **Migration** (one-way, tested): `renderAcceleration=cpu` maps to `renderBackend=cpu`. `renderAcceleration=gpu` maps to `renderBackend=opengl`, not `auto`, so an existing opt-in keeps its meaning.
- **Default: `cpu`.** `auto` becomes the default only after the phase 3 gate passes.
- **`vulkan` is accepted but reports `unavailable` until phase 4.** Do not make it silently fall back without telling the user, because that hides a missing feature.
- **Env:** `FLAMEWM_GPU=0|1` keeps its current meaning and still wins. Add `FLAMEWM_RENDER_BACKEND` only for testing.

### Settings UI placement

The control belongs on the page that already hosts `renderAcceleration` (`app_impl_3.rs:85`). Change it from a text field to a choice control with options `CPU`, `Auto`, `OpenGL`, and `Vulkan (not available)` until phase 4. Show a status line from the probe, for example `Active: OpenGL, <renderer string>` or `Active: CPU (no usable GPU: software renderer)`. The settings files are outside this document's scope, so the exact page and widget are for the implementing change to confirm.

### Detection and fallback chain

X11 session:

1. `FLAMEWM_GPU=0` returns `cpu`, regardless of the setting.
2. `renderBackend=cpu` returns `cpu`.
3. `opengl` or `auto`: load `libEGL` and `libGLESv2` with `dlopen`, then pick a display. Prefer the surfaceless platform, then the device platform (existing loader order). Create a context.
4. **Reject software renderers.** Renderer string containing `llvmpipe`, `softpipe` or `swrast` is not GPU. Keep the existing rule.
5. `auto` returns `opengl` on success and `cpu` otherwise. It never returns `vulkan` before phase 4.
6. Any later GPU error sets the slot to `Failed` and runs on CPU. The GPU is retried only when the setting changes (existing behaviour).

Wayland session:

- Smithay's `GlowRenderer` is the compositor renderer. Report `gles` as the active backend.
- `renderBackend=cpu` on Wayland has no compositor path in source (not verified). Log one line and ignore it. Do not claim a CPU compositor.
- `vulkan` on Wayland is not offered. See phase 5.

Probe budget: the probe runs once at start and on setting change. Target under 50 ms. Measure it before accepting that target.

### Crate ownership

| Crate | Owns | Change |
|---|---|---|
| `flamewm-api` | Backend vocabulary `RenderBackend`, parse, `renderAcceleration` migration. Pure, shared by both sessions and settings. | New in phase 1 |
| `flamewm-render-core` | Backend-neutral `PixelBackend` trait. Today it is in `render-x11/src/accel/mod.rs`. | Move in phase 1 |
| `flamewm-render-x11` | Selection, CPU path, GL path (`accel/gles*`). Later, the EGL window surface. | Phases 2 and 3 |
| `flamewm-ui-x11` | Glue: `apply_render_acceleration` becomes `apply_render_backend` (`lib.rs:50`). | Phase 1 |
| `flamewm-wm-wayland` | Reports `gles`. No change to the renderer. | Phase 2 status only |
| `flamewm-settings` | The choice control and status line. | Phase 2 |
| `flamewm-render-vulkan` (new) | Vulkan X11 swapchain behind a default-off `vulkan` feature. | Phase 4 only, if ever |

Keep all new FFI in named modules. `docs/ARCHITECTURE_EXCEPTIONS.toml` allows explicit paths only, with no wildcards, so every new unsafe module needs its own entry.

### X11 surface path

- **Today:** GL draws offscreen and results return to the CPU per op. This explains the small-op losses.
- **GLX:** not recommended. EGL works on X11 and is the path already loaded.
- **EGL window surface (`EGL_PLATFORM_X11`):** required for a GPU-resident frame. On Mesa, `eglSwapBuffers` goes through DRI3 and Present, so no separate DRI3 code is needed for this path. This is the change that could win for whole frames. It replaces the XRender presentation for GPU mode, so it is large.
- **Vulkan on X11:** `VK_KHR_xcb_surface` with the swapchain from the Mesa WSI, which uses DRI3 and Present underneath. This is more FFI than the EGL path with no measured gain yet.

### Wayland surface path

- **Today:** Smithay GLES renders the output. This is the only compositor renderer in source.
- **Vulkan:** would mean replacing `GlowRenderer` with a Vulkan renderer. Smithay 0.7 has no Vulkan renderer module (see the table above), so this is a rewrite of the render path. Not recommended.

### Memory, idle CPU and build cost

| Cost | Measured | Implication |
|---|---|---|
| Desktop PSS, GPU on | +143 MB (168,867 vs 25,670 kB) | Fails the no-extra-RAM goal. Default stays CPU. |
| WM and explorer PSS | Within 200 kB | GPU cost sits in the desktop process only. |
| Idle CPU ticks and wakeups | Equal on and off | No idle regression measured. Keep the no-idle-paint rule in `state.rs`. |
| Cold build | 878 s wall, about 290 external crates, `CARGO_BUILD_JOBS=2` ([BUILD_TIME_2026-11-05.md](BUILD_TIME_2026-11-05.md)) | Smithay and `glow` are already built, so phase 2 adds no crates. |
| Vulkan (`ash`) | Not measured | Expected to add a few crates. Must be measured before any merge. |
| `wgpu` | Not measured | Large graph. Do not add. |

Vulkan memory is not measured. The driver load on the EGL path already costs about 143 MB, so Vulkan should be assumed to cost at least that until measured.

## Rollout

Each phase is a small PR with its own acceptance test. Harness rules apply: one Cargo owner at a time, `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=2`.

| Phase | Scope | Size | Acceptance |
|---|---|---|---|
| **1** | `RenderBackend` enum and parse in `flamewm-api`. Migration from `renderAcceleration`. Move `PixelBackend` to `flamewm-render-core`. Read `renderBackend` in `ui-x11`, mapped to the existing mode. **No behaviour change.** | S | `cargo test -p flamewm-api` exits 0, with unit tests for each parse value and each migration case. `cargo test -p flamewm-render-x11` exits 0. `cargo check --workspace --all-targets` exits 0. Runtime: start with `FLAMEWM_GPU=0` and with `renderAcceleration=gpu`, each shows the expected log line. |
| **2** | Probe with three results: `Ready(renderer)`, `Software(renderer)`, `Missing(reason)`. One log line. Settings choice control and status line. Wayland reports `gles`. | S to M | Unit tests with mocked probe results for llvmpipe, missing `libEGL` and a usable GPU. Runtime on an Xvfb host shows `Software` and selects CPU. Runtime on an NVIDIA host shows `Ready`. Both need fresh runs. |
| **3** | EGL window surface and a GPU-resident frame batch. Only this phase can justify `auto`. | L | **Set the gate before measuring.** Proposed: frame-batch median at or below CPU median on the same workload; desktop PSS delta at most +20 MB; idle CPU ticks 0 over 15 s; pixel difference within the current 72-pixel tolerance. If any fails, stay on CPU and do not ship `auto`. |
| **4** | Optional. `flamewm-render-vulkan` with `ash`, behind a default-off `vulkan` feature. Only if phase 3 fails for a reason Vulkan fixes. | XL | Rejects software ICDs (`llvmpipe`, `lavapipe`). Measured cold-build delta and PSS. No default change. |
| **5** | Wayland Vulkan. Not recommended. | XL | Would need a Smithay renderer replacement. Do not start without a separate plan. |

### Phase 1 in detail (recommended next step)

1. Add `RenderBackend { Auto, Cpu, OpenGl, Vulkan }` and `parse` to `flamewm-api`. `parse` is pure. Unknown values fall back to `cpu` and log one line.
2. Add the migration function: `renderBackend` if present, else from `renderAcceleration` (`cpu`→`Cpu`, `gpu`→`OpenGl`), else `Cpu`.
3. Move the `PixelBackend` trait to `flamewm-render-core`. Re-export it from `render-x11` so callers do not change.
4. Route `apply_render_acceleration` through the migration. The active mode must not change for any existing setting.
5. Tests: `flamewm-api` unit tests for each value and each migration case. One test that `renderAcceleration=gpu` resolves to `OpenGl`. One test that the env override still wins.

## Risks

- **Driver or context loss mid-frame.** The per-op fallback exists. Phase 3 must keep the same guarantee for whole frames.
- **RAM.** +143 MB measured. Phase 3 can only ship if its gate holds.
- **Unsafe growth.** `render-x11` already has 1,313 `unsafe` tokens (see [QUALITY_SCORECARD.md](QUALITY_SCORECARD.md)). New FFI goes in named modules with explicit exception entries.
- **One GPU tested.** Behaviour on AMD and Intel drivers is unknown.
- **No GPU in CI.** Tests must use mocked probes. Runtime claims need a local GPU run.
- **Two keys.** `renderAcceleration` and `renderBackend` can disagree. Migration must be one-way and tested, and `renderBackend` must win once it is written.
- **Settings type change.** Text to choice control must keep existing stored values working.

## Open questions

1. Does a CPU compositor path exist in `wm-wayland`? Not found in the grep. If not, `renderBackend=cpu` on Wayland is a no-op and should say so in the UI.
2. Is the GPU blur result read back to the CPU per op? The contract implies it, but the readback call was not traced.
3. Is `accel/gles*` already listed in `docs/ARCHITECTURE_EXCEPTIONS.toml`? Not checked in this review.
4. Does the Vela docs' Vulkan requirement have a CPU fallback anywhere? Only the installation doc was read.

## Not checked

- No source was built or run for this document. Every number comes from the existing measurement docs or the commands listed above.
- Vulkan build cost, Vulkan PSS, and any AMD or Intel behaviour are unmeasured.
- The RustWebRender match count for GL and Vulkan terms was 1 file (`xrender.rs`), which is XRender and not a GPU backend.
