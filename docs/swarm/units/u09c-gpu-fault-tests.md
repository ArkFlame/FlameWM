# Unit u09c-gpu-fault-tests

Status: **UNBUILT** — Rust source and documentation were edited by static reading only. No cargo,
build, test, git operation, python or runtime session was run in this unit; compilation, test
execution and commit belong to the orchestrator.

Area: fault-injection tests for the X11 accel helper's hardware GL -> software GL -> CPU raster
chain. Owned paths: `crates/flamewm-render-x11/src/accel/mod.rs`,
`crates/flamewm-render-x11/src/accel/fault_tests.rs`, this file. `accel/helper.rs` was read, not
edited. Routes loaded: `flamewm-source-first`, `flamewm-rust-systems`,
`flamewm-parallel-execution`, `flamewm-semantic-modularization`, `lean-code`, `no-adhoc-scripts`.
No new owner or feature boundary was introduced: the change stays inside the existing `accel`
module.

## Seams added (production call sites unchanged)

`open_gpu()` in `accel/mod.rs` now delegates to `open_gpu_with(spawn)`, which takes
`&dyn Fn(helper::GpuMode) -> Result<Box<dyn GpuPixels>, String>` instead of calling
`helper::RemoteGpu::spawn` directly. Production still passes the real spawn in one closure; the
tier order, software-renderer detection, `gpu_failure_record` lines and status writes are untouched.

`try_gpu()` now delegates to `try_gpu_with(op, acceleration, open)`, which takes the acceleration
request as a value and the slot opener as `&dyn Fn() -> GpuSlot`. Production passes
`render_acceleration()` and `open_gpu`. Tests pass `RenderAcceleration::Gpu` explicitly, so no test
flips the process-wide setting that the CPU-path tests in `mod.rs` also touch.

`fault_tests.rs` is declared as `#[cfg(test)] mod fault_tests;` in `accel/mod.rs`.

## Test design

`FakeHelper` implements `GpuPixels`/`PixelBackend` with a fixed device name and an optional op
error, so a child that started but then died or timed out is reproduced without a GPU or a spawned
process. Each test resets this thread's GPU slot to `Untried` and holds a file-local mutex for its
status assertions.

| Test | Injected fault | Assertions |
| --- | --- | --- |
| `missing_helper_binary_falls_through_both_tiers_to_cpu` | Spawn returns `Err("flamewm-gpu-helper not found ...")` on both calls | Hardware then Software were both attempted; `GpuSlot::Failed`; `active=Cpu`, `device=None`, `fallback=InitFailed(...)`; no panic |
| `helper_exiting_nonzero_at_start_fails_hardware_then_keeps_software_device` | Hardware spawn returns `Err("helper exited with status 1 at start")`, Software spawn returns a `FakeHelper` llvmpipe device | Both tiers attempted; `GpuSlot::Ready` with the software device; `active=OpenGl`, `fallback=SoftwareDevice(...)`; no panic |
| `helper_timeout_or_death_mid_session_falls_back_to_cpu` | Hardware spawn succeeds; its first `paint_batch` returns `Err("helper reply timed out")` and then `Err("helper exited mid-session")` | No software retry; `try_gpu_with` returns `false`; frame untouched; `GpuSlot::Failed`; `active=Cpu`, `device=None`, `fallback=DeviceLost(...)`; CPU tier then paints the frame; no panic |

The failure-record lines themselves (`gpu_failure_record`) are exercised by the same code paths but
are not captured textually; the recorded status is asserted instead.

## Verification

Not run, by task rule (no cargo, no builds, no tests, no git). The edited regions were checked by
reading source only. Compilation, the three tests and their interaction with the existing accel
tests remain **UNVERIFIED** until the orchestrator builds and runs them.

PARTIAL — all three requested fault cases and the minimal injectable seams are written, but no
test or build was executed here, so correctness and cross-test status races are not proven.

## Follow-ups

Re-run the accel unit tests after any change to `open_gpu_with` or `try_gpu_with`. The
status assertions share the global `RenderBackendStatus` with the existing `accel/mod.rs` tests;
if parallel runs prove flaky, serialize those tests with a shared test-only mutex. No commit, push
or build was performed here; the orchestrator owns those steps.
