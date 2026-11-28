# z05 GPU fallback chain: default-mode record design

STATUS: DESIGN DELIVERED; step 1 implemented UNBUILT.

Scope: `crates/flamewm-render-x11/src/accel/{mod.rs,backend.rs,fault_tests.rs}` only. No new
dependency, no visual change, no new public API.

## Current structure (file:line, after step 1)

- `crates/flamewm-render-x11/src/lib.rs:3,20-24` — `accel` is private; the typed facade re-exports
  `RenderAcceleration`, `render_acceleration`, `render_backend_status`, `apply_render_backend_setting`
  and the pixel ops. Product code never imports `accel` internals.
- `crates/flamewm-render-x11/src/accel/backend.rs:10-17,19-27` — `RenderBackend::OpenGl` is the only
  request mapped to `RenderAcceleration::Gpu`; `Cpu`, `Auto` and `Vulkan` map to CPU, which is the
  product default (`flamewm-api/src/render_backend.rs:11-24`).
- `crates/flamewm-render-x11/src/accel/mod.rs:266-269` — `render_acceleration()` reads the atomic
  request; default zero means CPU. `:301-306` caches the one renderer probe; `:341-346` records a
  CPU hand-off in `RenderBackendStatus`; `:351-355` emits `FLAMEWM-GPU-FALLBACK stage=... status=fail
  cause=... blast_radius=...` only after a stage was actually attempted.
- `crates/flamewm-render-x11/src/accel/mod.rs:356-375` (step 1) — one-shot `CPU_SKIP_RECORDED` guard
  and `record_cpu_skip_once()` emitting `stage=cpu-default status=skip
  cause=render-acceleration-cpu blast_radius=works:cpu-raster|degraded:hardware-gl,software-gl`;
  a `#[cfg(test)]` thread-local counter makes the line assertable without stderr capture.
- `crates/flamewm-render-x11/src/accel/mod.rs:389-421` — `open_gpu()` / `open_gpu_with()` inject the
  helper spawn: hardware GL, then software GL, then `GpuSlot::Failed`. `:397` is the fault seam.
- `crates/flamewm-render-x11/src/accel/mod.rs:443-475` — `try_gpu_with()` is the single typed gate for
  blur, `paint_batch` and overlays; `:448-451` returns `false` in CPU mode so the caller keeps CPU
  pixels. Step 1 calls the skip record at `:449` before that return.
- `crates/flamewm-render-x11/src/accel/mod.rs:477-479` — small blur (`< 64*64`, `radius < 0.5`) also
  bypasses the gate, silently. `:550-557` routes batches through the same gate.
- `crates/flamewm-render-x11/src/accel/fault_tests.rs:1-3,9,66-68` — fault-injection tests use the
  spawn/selection seams, share `tests::TEST_SERIAL`, and never spawn a real GL helper. Step 1 adds
  `cpu_default_mode_skips_gl_probe_and_records_once()` at `:70-99`.

## Counted problems

1. `mod.rs:448-451` (pre-step-1) returned `false` in CPU mode with no structured diagnostic, so a
   default CPU session had zero `FLAMEWM-GPU-FALLBACK` evidence.
2. `mod.rs:351-355` only reports stages after a probe; no line explained *why* GL was never probed.
3. `fault_tests.rs` (pre-step-1) covered helper failure, software GL and mid-session death, but not
   the CPU-mode gate; a regression could probe GL in default mode unnoticed.
4. `mod.rs:477-479` silently skips GPU for small blur; that distinct reason has no record either.
5. `mod.rs:301-306` caches `PROBE` process-wide, so any accidental early probe would be sticky and
   invisible; the skip path must prove the probe seam stays untouched.

## Target design (typed facade remastering)

`accel` remains the single semantic owner. Records stay private inside `accel`; the typed facade in
`lib.rs` is unchanged, so WM, settings UI and reactor consumers keep calling the same re-exported
functions. A typed reason vocabulary (`stage`, `status`, `cause`, `blast_radius`) replaces ad-hoc
strings only where a record is emitted; `RenderBackendStatus` is not modified by the skip record, so
the status line and pixels are unchanged. `record_cpu_skip_once()` is idempotent per process via an
`AtomicU8`, emits one line, and is reached only from the CPU-mode branch of `try_gpu_with()`. Fault
tests inject an `open` closure that panics if GL is touched and a `#[cfg(test)]` counter for the
record, so the claim "no GL probe, one record" is checkable without a display or GL driver.

## Steps (each compiles alone)

1. **Implemented (step 1, UNBUILT).** Add `CPU_SKIP_RECORDED`, `record_cpu_skip_once()` and the call
   at the CPU gate in `mod.rs`; add `cpu_default_mode_skips_gl_probe_and_records_once()` in
   `fault_tests.rs`. Compiles alone: private items, existing imports only.
2. Replace the hard-coded skip cause with a typed `CpuSkipReason` enum in `accel` (e.g. `DefaultMode`,
   `EnvOptOut`, `SettingsCpu`), derived from `flamewm_api::render_backend::RenderBackend` choice; the
   record's `cause` token comes from the enum. No facade or call-site change.
3. Give the small-blur bypass (`mod.rs:477-479`) its own one-shot record with `stage=blur-small` and
   a separate guard, so step 1's once-line stays about CPU mode.
4. Surface the skip reason through a new typed `RenderBackendStatus` field via the existing
   `render_backend_status()` facade (one owner, one field), updating only status consumers that opt
   in; no pixel-path change.
5. Mirror the record into the session chain log behind the existing `FLAMEWM-SESSION-CHAIN` facade,
   reusing the step-2 reason tokens with a `works:cpu-raster|degraded:...` blast radius.

## Ported reference (path + choice)

- Path: `.vendor/kwin-master/src/compositor.cpp:262-268` (`attemptOpenGLCompositing()` returns false
  before constructing `EglBackend` when no render device is selected, with one
  `qCWarning(KWIN_CORE, "Found no render device!")`), `:292-297` (single GL creation call site), and
  `:218-219` (software render device as an explicit fallback tier).
- Choice ported: the *early, single, greppable skip record at the gate before any GL context is
  created*. KWin treats no-device as fatal; FlameWM's owner instead continues on CPU, so the port is
  the diagnostic shape and gate placement, not the fatal exit and not any upstream code. Reference
  material is read-only evidence, never a dependency or runtime input.

## Rollback

Revert `mod.rs:356-375` and `:449`, then remove `fault_tests.rs:70-99`; no other file, no Cargo
manifest and no facade signature is touched by step 1, so rollback restores the prior silent gate
exactly. Later steps revert independently in reverse order.

## Verification limit

UNBUILT by handoff: no `cargo`, test run, or runtime observation was performed. Static inspection
only; the fault test's assertions are written but unexecuted.
