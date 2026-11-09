# GPU Backend Spec: `renderBackend` = CPU | OpenGL | Vulkan

Status: implementation spec, 2026-11-08, base `origin/main` including phase 1 (commit `e29a7785`, pushed as `759f8b77`). Supersedes the design sections of [GPU_BACKENDS.md](GPU_BACKENDS.md); measurements stay in [GPU_FRAME_PAINT.md](GPU_FRAME_PAINT.md). Architecture rules: [ARCHITECTURE.md](ARCHITECTURE.md), `tools/architecture_guard.py`.

Evidence tags: **[src]** read in source this session, **[meas]** measured and recorded in `GPU_FRAME_PAINT.md`, **[decision]** chosen here, **[assumed]** not verified, with the check that would verify it.

## 1. Goals, non-goals, budgets

Goals
1. One user setting that selects the pixel renderer: `cpu`, `opengl`, `vulkan`, plus `auto`.
2. `cpu` stays the default and costs nothing: no GPU library loaded, no GPU thread, no probe.
3. Selecting a GPU backend never changes how the UI looks. CPU output is the reference.
4. Every failure degrades to a working CPU frame, with one log line and a status the user can read.
5. Switching is live on X11 and frees GPU memory when switching away.

Non-goals
- No change to blur, surface or outline look, parameters or layering.
- No Vulkan WSI, swapchain or presentation. Vulkan is headless compute, like the existing surfaceless GLES path.
- No GPU requirement. No new default dependency. No change to default build time or binary size.
- No Vulkan path in the Wayland compositor (section 5).

Hard budgets (a phase fails its gate if any is missed)

| Budget | Value | Source |
|---|---|---|
| `cpu` idle PSS, session | no increase over 23.8 MiB | [meas] `docs/QUALITY_SCORECARD.md` |
| `cpu` idle CPU ticks and wakeups | no increase | [meas] GPU_FRAME_PAINT.md live probe |
| `cpu` code path | zero new allocations and zero `dlopen` calls | [decision], tested in 8.2 |
| Switch GPU to CPU | desktop-process PSS returns to within 5 MiB of the `cpu` baseline within 2 s | [decision] (OpenGL costs +143 MB [meas], so this is the proof it is freed) |
| OpenGL or Vulkan frame batch vs CPU | median at least 3x faster, otherwise `auto` never selects it | [decision]; OpenGL measures 87.8 to 14.7 ms, 6.0x [meas] |
| Pixel equivalence | 8.1 | [decision] |

## 2. Settings schema

| Key | Type | Values | Default | Serialized when default |
|---|---|---|---|---|
| `renderBackend` | text | `cpu`, `auto`, `opengl`, `vulkan` | `cpu` | no |
| `renderAcceleration` | text | `cpu`, `gpu` | `cpu` | no; legacy, read for migration only |

State today [src]: phase 1 added `render_backend: String` to `ProductSettings` (`flamewm-platform/src/settings.rs`, `stable_settings!` row `(false, appearance, "renderBackend", render_backend)`), validation in `product_validate.rs`, and the pure resolver `flamewm_api::render_backend::{RenderBackend, choose, choose_from_snapshot}`.

### 2.1 Defect found in phase 1: stale legacy key re-enables the GPU

`choose(None, Some("gpu"))` returns OpenGL when `renderBackend` is absent. `renderBackend=cpu` is the default, so it is never serialized. Sequence: user has `renderAcceleration=gpu`, opens Settings, picks CPU, the UI writes `renderBackend=cpu`, which the serializer drops as default, so on the next load the legacy `gpu` applies again. The user's choice is lost.

Fix, **[decision]** migrate at load in the platform crate, not in the render crate:

```rust
// flamewm-platform/src/settings/render_backend_migrate.rs (new, ~25 lines)
/// One-way: a stored legacy `renderAcceleration=gpu` becomes `renderBackend=opengl`
/// and the legacy field is reset to its default so it is never written again.
pub(super) fn migrate_legacy_acceleration(s: &mut ProductSettings, backend_was_stored: bool) {
    if !backend_was_stored && s.render_acceleration == "gpu" {
        s.render_backend = "opengl".to_owned();
    }
    s.render_acceleration = "cpu".to_owned();
}
```

`backend_was_stored` comes from the parse step (key present in the file). Hook is one line in the load path next to `product_validate`. After migration, `choose()` keeps its legacy branch only for snapshots that lack `renderBackend` entirely (older settings clients), which is harmless.

Tests, all in `flamewm-platform/src/settings/tests.rs`:
- `legacy_gpu_migrates_to_opengl`: file `renderAcceleration=gpu` loads with `render_backend == "opengl"`.
- `legacy_gpu_then_cpu_choice_stays_cpu`: load that file, set `render_backend = "cpu"`, serialize, reload, assert `cpu` and assert the text contains neither key.
- `explicit_backend_wins_over_legacy`: both keys present, `renderBackend=cpu` stays cpu.
- `non_default_backend_round_trips`: `opengl`, `vulkan`, `auto` serialize as `renderBackend=<v>` and reload equal.

### 2.2 Environment overrides

Precedence, first match wins [decision]:
1. `FLAMEWM_GPU=0` forces `cpu` (existing opt-out [src] `accel/mod.rs` `resolve_env_override`).
2. `FLAMEWM_RENDER_BACKEND=cpu|auto|opengl|vulkan` (new, for tests and bug reports).
3. `FLAMEWM_GPU=1` forces `opengl` (existing).
4. The stored setting.

Unknown env values are ignored with one log line.

## 3. Typed API and ownership

One owner per mechanism. `flamewm-api` owns the vocabulary, `flamewm-render-x11::accel` owns the runtime, `flamewm-render-core` owns the `PixelBackend` trait [src] `pixels.rs:414` (`blur_rgba`, `fill_rounded_rect`, default `paint_batch`). No new crate edge is needed for phases 1 to 3.

### 3.1 Vocabulary (`flamewm-api/src/render_backend.rs`, exists)

Keep `RenderBackend { Cpu, Auto, OpenGl, Vulkan }`, `parse`, `as_setting`. Add:

```rust
/// What the running process actually uses, published for the status line and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderBackendStatus {
    pub requested: RenderBackend,
    pub active: RenderBackend,          // Cpu | OpenGl | Vulkan, never Auto
    pub device: Option<String>,         // GL_RENDERER or VkPhysicalDeviceProperties.deviceName
    pub fallback: Option<FallbackReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    NotBuilt,                 // vulkan requested, binary has no vulkan feature
    LibraryMissing(String),   // libEGL / libGLESv2 / libvulkan not found
    SoftwareDevice(String),   // llvmpipe, softpipe, swrast, VK_PHYSICAL_DEVICE_TYPE_CPU
    InitFailed(String),
    DeviceLost(String),       // after init
    ForcedByEnv,
}
```

`choose()` changes meaning of the Vulkan arm: it no longer silently maps to OpenGL on its own. It returns the *request chain*, and the runtime resolves it:

```rust
/// Ordered backends to try. The last entry is always Cpu.
pub fn chain(requested: RenderBackend, vulkan_built: bool) -> &'static [RenderBackend] {
    use RenderBackend::*;
    match (requested, vulkan_built) {
        (Cpu, _) => &[Cpu],
        (OpenGl, _) => &[OpenGl, Cpu],
        (Vulkan, true) => &[Vulkan, OpenGl, Cpu],
        (Vulkan, false) => &[OpenGl, Cpu],          // reported as FallbackReason::NotBuilt
        (Auto, true) => &[Vulkan, OpenGl, Cpu],
        (Auto, false) => &[OpenGl, Cpu],
    }
}
```

`Auto` keeps phase-1 behaviour (`Cpu` with a reason) until the phase-3 gate passes; after the gate it uses `chain`.

### 3.2 Runtime trait (`flamewm-render-x11/src/accel`)

```rust
/// A GPU pixel backend. `PixelBackend` stays the op contract: on error the frame is untouched.
pub(super) trait GpuPixels: PixelBackend {
    fn kind(&self) -> RenderBackend;     // OpenGl | Vulkan
    fn device(&self) -> &str;
}
enum GpuSlot { Untried, Ready(Box<dyn GpuPixels>), Failed }
```

`gles::GpuBlur` implements it with `kind() = OpenGl` (existing `renderer()` becomes `device()`). `try_gpu` is unchanged apart from calling `Box<dyn GpuPixels>` and tagging the status. One `dyn` call per op, negligible next to an upload.

### 3.3 Status API

Replace the consumers' use of `AccelerationStatus` with `render_backend_status() -> RenderBackendStatus`. Keep `render_acceleration_status()` as a thin shim so `flamewm-ui-x11` and the WM keep compiling; delete it when no caller is left. `apply_render_backend_setting(&SettingsSnapshot)` (exists, `accel/backend.rs`) remains the only entry point used by `flamewm-ui-x11/src/lib.rs` and `flamewm-wm-x11/src/live_settings.rs`.

## 4. Selection state machine (X11)

States per thread: `Untried`, `Ready`, `Failed`. Process-wide: `REQUESTED` (atomic), `GENERATION` (atomic u32).

```
set_backend(req):                       // called from apply_render_backend_setting, any thread
  eff = env_override(req)               // 2.2
  if eff == previous: return
  REQUESTED = eff; GENERATION += 1      // invalidates every thread's slot
  log "render backend: <req> -> <eff>"

try_gpu(op):                            // hot path, per pixel op
  if REQUESTED is Cpu: return false     // one relaxed atomic load, nothing else
  slot = GPU.get()
  if slot.generation != GENERATION: drop(slot.ready); slot = Untried(GENERATION)
  if Untried: for b in chain(REQUESTED): try init(b)
        ok    -> Ready (record status)
        Err   -> record FallbackReason, continue
      none ok -> Failed (record), return false
  run op; Err -> DeviceLost / op error: Failed (until next set_backend), return false
```

### 4.1 Defect found in the current code: GPU memory is not freed on switch

[src] `set_render_acceleration` resets only a `Failed` slot, and only on the *calling* thread (`GPU` is `thread_local!`). Switching GPU to CPU leaves `GpuSlot::Ready` alive in every thread that painted, so the +143 MB is never returned until exit. The generation counter above fixes both problems: the next `try_gpu` on each thread sees a stale generation and drops its slot. Threads that never paint again keep their slot, so `set_backend` also drops the calling thread's slot eagerly, and the WM/desktop paint threads call `accel::release_if_stale()` (one atomic compare, from the existing idle hook) so an idle thread frees within one idle tick. Acceptance in 8.3.

### 4.2 Probing rules

- `cpu`: nothing runs. No probe, no `dlopen`.
- First GPU request: init happens lazily on the first op that is large enough (`MIN_GPU_PIXELS = 64*64` [src]), not at startup.
- Software devices are rejected: GLES renderer string contains `llvmpipe|softpipe|swrast` [src]; Vulkan `VK_PHYSICAL_DEVICE_TYPE_CPU` or name containing `llvmpipe|lavapipe`.
- Test override `FLAMEWM_ALLOW_SOFTWARE_GL=1` accepts software devices so CI can run the equivalence test (8.1). It is read only in `cfg(test)` and examples, never in the shipped path.
- A failed backend is not retried until the setting or env changes (existing rule [src]).
- Logging: one `eprintln!` per transition, prefix `flamewm-render-x11:`, same style as today.

### 4.3 Settings app status line

The Settings process must not load GPU libraries to show a status. **[decision]** the page runs `current_exe() --gpu-probe` as a child on explicit click of a "Check" button. The child tries `chain(Auto)` once, prints one line `backend=<opengl|vulkan|cpu> device=<name> reason=<text>` and exits, so the libraries die with it. The page shows the line next to the control. The live status of the running WM is shown from `render_backend_status()` published through the existing settings snapshot value `renderBackendStatus` (read-only, text), written by the WM on each transition. Cost: about 40 lines, no persistent memory.

## 5. Per-session mapping

| Session | `cpu` | `opengl` | `vulkan` | Live switch |
|---|---|---|---|---|
| X11 (`flamewm-render-x11`) | `CpuPixels` (XRender, Xft, `gaussian_blur_rgba_with`) [src] | `gles::GpuBlur` via dlopen `libEGL.so.1`, `libGLESv2.so.2` [src] | new `vulkan::VkPixels`, feature `vulkan` | yes |
| Wayland (`flamewm-wm-wayland`) | optional, see below | `GlowRenderer` (smithay `renderer_glow`) [src] | not offered | no, restart |

### 5.1 Vulkan on X11

Decision: build it, behind a default-off cargo feature, and ship it only if the section 5.3 criterion is met.

- Crate: `crates/flamewm-render-vulkan` (new, leaf, depends on `flamewm-render-core` only). It exports `VkPixels: PixelBackend`. It must be added to `docs/ARCHITECTURE.md` crate table and pass `tools/architecture_guard.py` with zero new violations; `flamewm-render-x11` depends on it only with `feature = "vulkan"`.
- Dependency: `ash = { version = "0.38", default-features = false, features = ["loaded", "std"] }`. `loaded` means `dlopen("libvulkan.so.1")` at runtime and no link-time dependency [assumed: confirm the ash 0.38 `Entry::load` API when implementing]. smithay's optional `backend_vulkan` already pins `ash 0.38.0` in the vendored tree [src], so version choice is consistent, but `Cargo.lock` has no `ash` entry today [src], so this is a new dependency, off by default.
- API level: Vulkan 1.1 minimum, no extensions required, compute queue only. Do not require 1.4 (Vela does, which excludes older GPUs; FlameWM must not).
- Shaders: GLSL compute `blur_h.comp`, `blur_v.comp`, `fill_rounded.comp` in `crates/flamewm-render-vulkan/shaders/`. SPIR-V is compiled by `tools/build_vulkan_shaders.sh` (`glslc`) and **committed** as `*.spv`, loaded with `include_bytes!`. No build script, no `glslc` at build time, so default and CI builds are unchanged. A CI check runs the script and fails on a diff.
- Buffers: one host-visible storage buffer sized to the frame plus one device-local ping-pong buffer, allocated on first op and re-allocated only when the frame size grows; freed when the slot drops. Single command buffer per `paint_batch`: upload, dispatch ops in order, copy back, one fence wait.
- Op semantics: identical to `PixelOp` [src] (`FillRoundedRect` with optional hole, `Blur`); sigma mapping identical to `blur_rgba` (`radius / 2.0` [src]).
- Device pick: first physical device that is not `TYPE_CPU`, preferring discrete over integrated; no user device selector in phase 4.
- Errors: any `vk::Result` error returns `Err(String)`; `ERROR_DEVICE_LOST` becomes `FallbackReason::DeviceLost`. The frame is untouched on error (copy-back only after the fence succeeds), same contract as `paint_batch` default [src].
- Safety: `unsafe` is confined to one module `vk.rs`, each block commented; no raw pointers leave the crate; handles are RAII-destroyed in reverse order; fds are not used (no external memory).

### 5.2 Wayland

[src] smithay in the vendored tree has `renderer/{gles,glow,pixman,multigpu}` and a `backend/vulkan` module that provides instance and physical-device helpers (`ash` 0.38), but no Vulkan *renderer*. FlameWM's compositor uses `GlowRenderer` (`flamewm-wm-wayland/src/render/mod.rs`). Therefore:
- `vulkan` on Wayland resolves to `opengl` with `FallbackReason::NotBuilt` and the status line says so. Writing a Vulkan smithay renderer is out of scope and not justified by any measurement.
- `cpu` on Wayland could use smithay `PixmanRenderer` (`renderer_pixman`, exists in vendored tree [src], not enabled in `flamewm-wm-wayland/Cargo.toml` [src]). That is a compositor renderer swap, not a pixel-op switch. **Go/no-go [decision]:** do it as phase 5 only if a prototype shows idle compositor PSS at least 20 MiB below the `GlowRenderer` build on the same machine, with golden-equal output on the visual tests. Until then `renderBackend=cpu` on Wayland logs one line (`wayland compositor always uses GLES`) and runs `GlowRenderer`. Do not claim a CPU compositor.
- Wayland backend changes need a compositor restart; the Settings UI shows "Applies after restart" for that session.

### 5.3 Vulkan ship criterion

Vulkan stays a build-time feature and the Settings option is shown only when the binary has it. It becomes part of release builds only when, on the same machine with the same frame data (`pixel_bench` extended in 8.3), on at least two GPU vendors:
1. `paint_batch` median is at most 0.9x the OpenGL median, **or** desktop PSS delta over `cpu` is at most 0.7x the OpenGL delta (143 MB [meas], so at most about 100 MB); and
2. equivalence (8.1) passes; and
3. no device-lost or validation errors in a 30 minute run with `VK_LAYER_KHRONOS_validation`.

If neither performance condition holds, Vulkan remains experimental and is never part of `auto`. This is the honest answer to "Vulkan vs OpenGL": [assumed] it may not beat GLES for blur and fills at these frame sizes, because both pay the same upload and readback; the gate decides, not the name.

## 6. Shared pixel contract (look must not change)

All backends implement `PixelBackend` [src]. CPU is the reference. Blur radius to sigma, rounded-rect inset (`row_inset` [src]), straight-alpha source-over rounding (`blend_span` [src]) are defined by the CPU path; GPU shaders match them.

## 7. Memory and lazy loading

- No GPU library is linked. EGL/GLES and Vulkan are `dlopen`ed on first GPU op [src for GLES, decision for Vulkan]. `cpu` never loads them.
- Slots hold the only GPU allocations. Dropping a slot calls the backend `Drop` (destroy context or device, free buffers). Because glibc `dlclose` of GPU drivers is unreliable, the freed-memory budget in 1 is verified by PSS measurement, not assumed; if drivers leave more than 5 MiB resident after drop, the fallback design is to run GPU work in a helper process (`flamewm-gpu-helper`, shm frame in, shm frame out) so the driver dies with it. That is only built if 8.3 fails. [assumed: confirm with the measurement]
- Measured (PSS probe `crates/flamewm-render-x11/examples/gpu_switch_pss.rs`, RTX 3050 Ti, NVIDIA driver): cpu baseline 8509 KiB, opengl active 143733 KiB, back to cpu 65369 KiB (65405 KiB after work). Slot memory is freed but about 57 MiB (56.9 MiB) of driver memory stays resident after `dlclose`. The 5 MiB gate FAILS; the helper-process fallback is triggered per this section, pending the cheaper mitigations in section 11.
- Scratch retention for the CPU path stays capped at `CPU_SCRATCH_RETAIN_BUDGET_BYTES = 4 MiB` [src].
- GPU frame buffers are capped at 3840x2160x4 bytes (about 33 MB); larger frames use the CPU path. [decision]

## 8. Test plan and gates

Build env for every command: `export PATH=/run/media/linsaftw/SDD/rust189/bin:$PATH CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-shared CARGO_BUILD_JOBS=2`, serialized through the shared flock `/tmp/opencode/flamewm-terra-cargo.lock`.

### 8.1 Equivalence

New `crates/flamewm-render-x11/tests/backend_equivalence.rs`. Renders a fixed scene (rounded fill, hole stroke, blur radius 12 and 28, 800x500, plus a batch) on CPU and on each available GPU backend and compares RGBA:

- Fills and strokes: max per-channel absolute difference at most `T_fill`.
- Blur: max per-channel difference at most `T_blur` and mean absolute difference at most `M_blur`.
- Thresholds are **calibrated first, not guessed**: phase 2 runs the existing GLES path against CPU, records the observed maxima in the test as `const` with a comment `observed <n> on <renderer>`, and sets each threshold to `observed + 1`. The product look is protected because the thresholds can only be loosened by editing a recorded number in review.
- Needs `FLAMEWM_ALLOW_SOFTWARE_GL=1` on CI (llvmpipe, lavapipe); skipped with a printed reason when no GL or Vulkan library exists.
- Appearance acceptance (`tools/flamewm_appearance_acceptance.py`, `tools/flamewm_visual_tests.py`) is re-run with `FLAMEWM_RENDER_BACKEND=opengl` on an own Xvfb and must produce the same pass set as `cpu`.

### 8.2 Zero-cost CPU

- Unit: `cpu` request makes `try_gpu` return `false` without touching `GPU` thread-local (counter test with a `#[cfg(test)]` init counter).
- Runtime: start desktop with defaults, read `/proc/<pid>/maps`, assert no `libEGL`, `libGLES`, `libvulkan`, `libnvidia`, `libGLX_mesa` mappings.

### 8.3 Live switch and memory protocol

On an own Xvfb or Xephyr (`pgrep -a Xvfb Xephyr` first, pick a free `:N` in 97..119, explicit `DISPLAY`, never `:0` or `wayland-0`, kill only your own PID):
1. Start session with `renderBackend=cpu`, wait 10 s, read `Pss` from `/proc/<pid>/smaps_rollup` for desktop, WM, shell. Record as `P_cpu`.
2. Set `renderBackend=opengl` via the settings IPC, drive a blur-heavy action, wait 10 s, record `P_gl`. Expect `P_gl - P_cpu` near +143 MB on a real GPU [meas].
3. Set `renderBackend=cpu`, wait 2 s, record `P_back`. Gate: `P_back - P_cpu <= 5 MiB`. Failure triggers the helper-process fallback in 7.
4. Repeat for `vulkan` when the feature is built.

Result (probe example, not the live Xvfb session): P_cpu 8509 KiB, P_gl 143733 KiB (+135 MiB), P_back 65369 KiB (+56.9 MiB vs P_cpu). Gate FAILED (limit 5 MiB).

`pixel_bench` (`crates/flamewm-render-x11/examples/pixel_bench.rs`) gains `--backend cpu|opengl|vulkan` and prints the same table as `GPU_FRAME_PAINT.md`; results are appended to that file with the GPU model.

### 8.4 Unit and integration commands (exit 0 required)

```
cargo test -p flamewm-api render_backend
cargo test -p flamewm-platform settings
cargo test -p flamewm-render-x11 accel
cargo test -p flamewm-render-x11 --test backend_equivalence          # phase 2
cargo test -p flamewm-render-vulkan                                   # phase 4, --features vulkan
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings                 # Rust 1.89.0, target-clippy189
python3 -I tools/architecture_guard.py                                # exit 0, no new violations
```

### 8.5 Fault injection

`FLAMEWM_RENDER_FAULT=init|op|lost` (test builds only) forces each `FallbackReason` so the state machine and the status line are tested without hardware: init failure goes to the next backend in the chain, op failure goes to CPU for that op, lost goes to `Failed` and the next frame is CPU-correct.

## 9. Work packages

Serialize packages that touch the same file; build commands are serialized by the shared flock.

| # | Package | Files (owner) | Depends on | Size | Gate |
|---|---|---|---|---|---|
| P1 | Phase 1 vocabulary and settings key | `flamewm-api/src/render_backend.rs`, `flamewm-render-x11/src/accel/{mod,backend}.rs`, platform settings rows, `flamewm-ui-x11`, `flamewm-wm-x11` | none | done, pushed (tests, clippy 1.89, guard, fmt exit 0 per its report) | 8.4 commands |
| P1b | Migration fix 2.1 | `flamewm-platform/src/settings/render_backend_migrate.rs`, one hook line, tests | P1 | S (~60 lines) | 4 tests in 2.1 |
| P2a | `GpuPixels` trait, status API 3.2, 3.3 | `accel/mod.rs`, `accel/gles.rs`, `flamewm-api` | P1 | S | accel tests, unchanged behaviour |
| P2b | Generation counter and free-on-switch 4.1, 4.2 | `accel/mod.rs`, `accel/backend.rs` | P2a | S | 8.2 unit, 8.3 steps 1 to 3 |
| P2c | Equivalence test and calibration 8.1 | `tests/backend_equivalence.rs` | P2a | M | calibrated constants recorded |
| P3a | Settings control and status line 2, 4.3 | `flamewm-settings/src/{actions,app_impl_2,app_impl_3}.rs`, `docs`; text field becomes choice | P1b | M | settings tests, screenshot on own Xvfb |
| P3b | `auto` enabled, default stays `cpu` | `flamewm-api` `chain`, `accel/backend.rs` | P2b, P2c | S | frame gate in section 1 |
| P4 | Vulkan crate and feature 5.1 | `crates/flamewm-render-vulkan`, `tools/build_vulkan_shaders.sh`, `docs/ARCHITECTURE.md`, `ARCHITECTURE_EXCEPTIONS.toml` only if the guard needs it | P2a | L | 5.3 criterion |
| P5 | Wayland pixman prototype 5.2 | `flamewm-wm-wayland` | none | M, optional | 20 MiB criterion |

P1b, P2a, P3a can run in parallel (disjoint files). P2b and P2c follow P2a. P4 and P5 are independent of each other. Do not enable `auto` as the default in any package; that is a separate decision after the section 1 gate.

## 10. Risks, rollback, security

| Risk | Mitigation |
|---|---|
| Driver memory not returned after slot drop | 8.3 gate, helper-process fallback (7) |
| Vulkan no faster than GLES | 5.3 gate; stays experimental |
| Pixel drift between backends | 8.1 calibrated thresholds, appearance acceptance on `opengl` |
| New dependency `ash` | optional, off by default, `loaded`, listed in `docs/DEPENDENCIES.md`, audited by `tools/audit-flamewm-dependencies.py` |
| Shader or driver bug crashes the process | Vulkan and GL run in-process; a crash is not recoverable, so `FLAMEWM_GPU=0` and a settings value of `cpu` are the documented opt-out, and `Failed` slots never retry. If field reports show driver crashes, move GPU work to the helper process (7) |
| Frame data exposure | no external memory or fd sharing; buffers freed on drop; shaders are first-party SPIR-V checked in |
| Rollback | each package is a separate commit; setting default is `cpu`, so reverting P3b or P4 changes nothing for default users |

## 11. Decisions that were open

| Question | Decision |
|---|---|
| Does `auto` default on? | No. Default `cpu` until the section 1 3x gate and 8.3 free-on-switch gate pass on two machines. |
| Should `vulkan` fall back silently? | No. It returns a `FallbackReason` and the status line states it; `NotBuilt` and `InitFailed` are distinct. |
| Hot switch or restart? | X11 live; Wayland restart. |
| Vulkan on Wayland? | Not offered, no smithay Vulkan renderer. |
| Keep legacy `renderAcceleration`? | Read for one-way migration at load, never written. |
| Where is the probe run? | Child process on click for Settings; lazy and in-process for the WM. |
| Vulkan version floor? | 1.1, no extensions. |
| Compile shaders at build time? | No, SPIR-V committed, script plus CI diff check. |

## Appendix: review log

Pass 1, gaps against source evidence. Found and fixed:
1. Stale legacy key re-enables GPU after the user picks CPU (2.1), a real defect in the phase-1 design.
2. `GPU` slot is per-thread, and only `Failed` is reset on switch, so GPU memory is not freed (4.1).
3. The earlier GPU_BACKENDS.md said `vulkan` reports "unavailable", while phase-1 code maps it to OpenGL; unified into the explicit chain with `FallbackReason` (3.1).
4. Smithay does have `backend_vulkan` and `renderer_pixman`; the old doc said only that no Vulkan renderer exists. Corrected in 5.2 and used for the Wayland CPU option.
5. `probe_gpu_renderer` caches by constructing a full `GpuBlur`, which would load drivers in the Settings process; replaced by a child-process probe (4.3).

Pass 2, ambiguity, testability, number sourcing. Fixed:
1. Pixel tolerance had no source, so it is now calibrated against the measured GLES-vs-CPU difference before it is fixed (8.1).
2. `auto` semantics conflicted between "best available" and "default cpu"; split into "auto = chain" and "default stays cpu" with a gate (3.1, 11).
3. "Frees memory" was unmeasurable; given a numeric gate (5 MiB in 2 s) and a protocol (8.3).
4. The Vulkan "better than GL" claim removed; replaced by the ship criterion (5.3).
5. Env precedence made total (2.2).

Pass 3, implementer walkthrough (can a CODER do P1b, P2a, P2b, P3a without asking?). Fixed:
1. P1b needs to know where "key was stored" comes from; stated (2.1).
2. P2b needs the idle hook name; stated as the existing idle hook, but the exact function is **not confirmed in source** (see below).
3. Package order and file disjointness listed (9).

Section scores (out of 100, reasons)

| Section | Score | Why not higher |
|---|---|---|
| 1 Goals and budgets | 93 | The 3x and 5 MiB gates are chosen, not derived; the 23.8 MiB figure is a nested-Xephyr measurement. |
| 2 Settings | 96 | Fully specified with tests; settings-snapshot status key `renderBackendStatus` not yet plumbed in code. |
| 3 API | 95 | Signatures are exact; `dyn` boxing cost not benchmarked (expected negligible). |
| 4 State machine | 94 | Idle-hook release function not confirmed; generation approach is sound but untested. |
| 5 Per-session | 88 | Vulkan crate details (ash 0.38 `loaded` API, memory type choice) are [assumed]; Wayland pixman gain is unmeasured. |
| 6 Pixel contract | 95 | Thresholds deliberately deferred to calibration. |
| 7 Memory | 90 | Driver unload behaviour is unmeasured; the helper-process fallback is a design, not a spec. |
| 8 Tests | 95 | CI has no GPU; equivalence depends on llvmpipe or lavapipe availability on the runner. |
| 9 Packages | 95 | Sizes are estimates. |
| 10 Risks | 92 | No field data on driver crash rates. |
| 11 Decisions | 96 | n/a |

Not claimed: this is not 100/100. Sections 5, 7 and 4 are below 95 because they depend on facts that need a build or a GPU to verify: the idle-hook function name, `ash` loaded-entry API, driver unload behaviour, and Vulkan versus GLES speed. Each is named with its check. Nothing here was compiled or run in this session; all `[src]` claims are file reads.
