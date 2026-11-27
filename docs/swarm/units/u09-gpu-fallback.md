# Unit u09-gpu-fallback

Status: **UNBUILT** — static source audit and Rust edits only. No cargo, build, test, git or runtime session was run in this unit; verification and commit belong to the orchestrator.

Area: GPU/renderer fallback chain in the X11 renderer. Owned paths: `crates/flamewm-render-x11/src`, this file. Task: find GPU/GL/EGL init paths that can fail hard (`unwrap`, `expect`, `panic`, `exit`) and make each degrade through hardware GL -> software GL -> CPU raster, logging one failure-record line (`stage`, `cause`, `blast_radius`) per failed stage.

## Audit rows (`file:line`, failure, fallback)

| `file:line` | Failure | Fallback |
| --- | --- | --- |
| `crates/flamewm-render-x11/src/accel/helper.rs:154` (prior attempt) | `serve_gpu_helper` called `gles::GpuBlur::new()` and aborted the child on GL init error; the parent saw a dead pipe instead of a reason | GL init failure is written back as a status-1 reply and the helper returns `Ok`, so the parent can read the cause and continue the chain; no child abort reaches the parent. |
| `crates/flamewm-render-x11/src/accel/helper.rs:46` (prior attempt) | One-shot `RemoteGpu::spawn()` opened only the default (hardware) GL device; no software stage existed | `spawn(mode: GpuMode)` with `Hardware`/`Software`; the `Software` mode sets `LIBGL_ALWAYS_SOFTWARE`, `MESA_LOADER_DRIVER_OVERRIDE=llvmpipe`, `GALLIUM_DRIVER=llvmpipe`, `EGL_PLATFORM=surfaceless` on the child only. |
| `crates/flamewm-render-x11/src/accel/mod.rs:361` (before edit) | `try_gpu` made a single `RemoteGpu::spawn()` attempt and took `GpuSlot::Failed` -> CPU on any error; the call no longer matched the new `spawn(mode)` signature, so the crate did not build | `try_gpu` now calls `open_gpu()`; the broken single-attempt call site is gone. |
| `crates/flamewm-render-x11/src/accel/mod.rs:369` | Hardware GL init failure was a hard stop to CPU; no second GL attempt | Stage 1 `spawn(GpuMode::Hardware)`; on `Err` emits one `stage=hardware-gl` failure record and continues to software GL. A hardware request that resolves to llvmpipe is kept as the software-GL result rather than discarded. |
| `crates/flamewm-render-x11/src/accel/mod.rs:392` | No software-GL stage existed; llvmpipe was detected only to be rejected to CPU | Stage 2 `spawn(GpuMode::Software)`; on success records a degraded software device and keeps rendering on the GPU child, on `Err` emits one `stage=software-gl` failure record and returns `GpuSlot::Failed` so the caller runs the CPU raster. |
| `crates/flamewm-render-x11/src/accel/mod.rs:349` | No structured failure evidence: only ad-hoc `eprintln!` lines with no stage, blast radius, or greppable tag | `gpu_failure_record(stage, cause, blast_radius)` emits exactly one `FLAMEWM-GPU-FALLBACK stage=<s> status=fail cause=<c> blast_radius=<r>` line per failed stage. |
| `crates/flamewm-render-x11/src/accel/mod.rs:357` | A software renderer was recorded as a full success (`fallback=None`) or thrown away to CPU | `record_software` keeps the GPU slot active while setting `fallback=SoftwareDevice(renderer)`, so the status line explains the degraded device. |
| `crates/flamewm-render-x11/src/bin/flamewm-gpu-helper.rs:7` | `std::process::exit(1)` on any `serve_gpu_helper` error turned a dead stdout pipe into a hard child exit | Removed the explicit exit; a write failure means the parent is gone, so the helper logs once and exits quietly with no status escalation. |

Non-findings (audited, left unchanged): `Egl::load`, `Gl::load`, `GpuBlur::new`, `create_on` and `finish` in `accel/gles{,.rs}` and `accel/gles/loader.rs` already return `Result` and propagate every EGL/GL failure; `probe_gpu_renderer` caches a `Result`; every `unwrap`/`expect`/`panic!` remaining in `accel/mod.rs` and `app/gpu_frame.rs` is inside `#[cfg(test)]` code. `bin/flamewm-render-x11.rs:22` is the top-level application entry, not a GPU init path.

## Implemented fallback chain

- `try_gpu` (on an `Untried` slot) calls `open_gpu()` exactly once per init attempt. `open_gpu` tries `Hardware`, then `Software`, then returns `Failed`; `try_gpu` then falls through to the caller's CPU raster and every op-level GPU error still degrades to CPU as before.
- Each failed stage emits one failure record: `FLAMEWM-GPU-FALLBACK stage=<hardware-gl|software-gl> status=fail cause=<error> blast_radius=<what works|what is degraded>`. Hardware failure names `degraded:next=software-gl`; software failure names `degraded:cpu-raster`.
- A usable software device is kept (not skipped to CPU), with `fallback=SoftwareDevice(...)` recorded so the Settings probe reports the degraded renderer.
- The GL driver still lives only in the child process; dropping or killing either attempt's child frees its device memory, so a failed hardware attempt cannot leak into the software attempt.

## Reference precedent

The child-side software request follows Mesa/EGL's standard surfaceless llvmpipe environment (`EGL_PLATFORM=surfaceless`, `GALLIUM_DRIVER=llvmpipe`), matching the layout in `accel/helper.rs`. No `.vendor` copy was consulted for this change; the chain is bounded to two GL attempts and one CPU stage, so a missing driver cannot produce a retry loop.

## Verification

Not run, by task rule (no cargo, no git, no builds, no tests). Changes were checked by reading the edited regions only. Compile correctness (the `GpuMode` argument now matches `spawn(mode)`), failure-record output, and actual hardware/software/CPU degradation remain **UNVERIFIED** until the orchestrator builds and runs the GPU backend tests on hardware.
