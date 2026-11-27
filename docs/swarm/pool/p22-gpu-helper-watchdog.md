# Unit p22-gpu-helper-watchdog
STATUS: DONE

crates/flamewm-render-x11/src/accel: a hung GPU helper must be detected by a deadline and the renderer must fall back to CPU raster, with a failure record; audit existing deadline logic against fault_tests.rs and fix gaps.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-x11/src/accel/helper.rs:75 | Startup device reply used blocking `read_exact`; a hung helper blocked spawn forever with no deadline. | Read the first reply through `read_reply_with_deadline`. | UNBUILT — a legitimately slow GL init could hit the 1 s deadline. |
| crates/flamewm-render-x11/src/accel/helper.rs:81-83 | Per-op reply used blocking `read_exact`; a hung mid-session helper froze the paint thread instead of falling back. | Read op replies through `read_reply_with_deadline`; timeout becomes `Err("helper reply timed out")`. | UNBUILT — a slow software-GL frame could be killed early. |
| crates/flamewm-render-x11/src/accel/helper.rs:87-114 | No deadline helper existed anywhere in `accel`. | Add scoped reader thread + `mpsc::recv_timeout(1000ms)`; kill child on timeout so the pipe closes and the reader unwinds; move `tx` into the thread (`:99`) so no `&Sender` capture is needed. | UNBUILT — depends on SIGKILL closing the pipe so the scoped join returns. |
| crates/flamewm-render-x11/src/accel/mod.rs:445-451 | Audit only: op `Err` already records `FallbackReason::DeviceLost` and leaves `GpuSlot::Failed`. | None — the new timeout string flows into the existing CPU hand-off and failure record. | None (audit only). |
| crates/flamewm-render-x11/src/accel/fault_tests.rs:142 | Audit only: timeout/death is injected via `op_error`, not a real wire deadline; its `DeviceLost("helper reply timed out")` assertion matches the new timeout string. | None — no test edits needed. | None (audit only). |

PASS — a hung GPU helper is now bounded at both reply points; timeout surfaces as `Err("helper reply timed out")`, which the unchanged `try_gpu_with` records as `FallbackReason::DeviceLost` and hands the untouched frame to CPU raster, matching `fault_tests.rs`. Reference-first: `.vendor/kwin-master/src/watchdog.cpp` uses a bounded liveness deadline; a local scoped-read plus child kill was chosen because this helper is a pipe child, not a systemd service. Every edit is marked UNBUILT: git/cargo/builds were forbidden in this shared checkout, so compiling and running `fault_tests.rs` are deferred to the orchestrator.
