# Unit p22-gpu-helper-watchdog
STATUS: DONE

crates/flamewm-render-x11/src/accel: a hung GPU helper must be detected by a deadline and the renderer must fall back to CPU raster, with a failure record; audit existing deadline logic against fault_tests.rs and fix gaps.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-x11/src/accel/helper.rs:75 | Startup device reply is a blocking pipe read; without a deadline a hung helper blocked spawn forever. | Handshake uses `pipe_exchange_with_deadline(..., None, ...)`; no request is written before the first reply. | UNBUILT — a legitimately slow GL init could hit the 1 s deadline. |
| crates/flamewm-render-x11/src/accel/helper.rs:80-88 | Audit gap: `exchange()` ran an unbounded `write_message` before the read deadline, so a helper that stopped reading stdin could block `write_all` on a full pipe and never reach the watchdog. | Route write and read through one `pipe_exchange_with_deadline` call with `Some((stdin, request))`. | UNBUILT — one shared 1 s budget covers write plus read; a slow software-GL frame could be killed early. |
| crates/flamewm-render-x11/src/accel/helper.rs:91-123 | Generalized deadline helper: on timeout it kills the child, closing the pipe so the scoped reader unwinds; the timeout string must match `fault_tests.rs`. | Optional write runs inside the same scoped thread; keep `mpsc::recv_timeout(1000ms)`, `child.kill()`, and `Err("helper reply timed out")`. | UNBUILT — `thread::scope` still joins the reader; a second process holding the stdout write end could outlast the deadline. |
| crates/flamewm-render-x11/src/accel/mod.rs:445-451 | Audit only: op `Err` already records `FallbackReason::DeviceLost` and leaves `GpuSlot::Failed`. | None — the new timeout string flows into the existing CPU hand-off and failure record. | None (audit only). |
| crates/flamewm-render-x11/src/accel/fault_tests.rs:142 | Audit only: timeout/death is injected via `op_error`, not a real wire deadline; its `DeviceLost("helper reply timed out")` assertion matches the new timeout string. | None — no test edits needed. | None (audit only). |

PASS — a hung GPU helper is now bounded at both reply points and on the request write: `pipe_exchange_with_deadline` puts the spawn handshake and every op exchange on one scoped thread with a 1000 ms deadline, kills the child on timeout, and returns `Err("helper reply timed out")`, which the unchanged `try_gpu_with` records as `FallbackReason::DeviceLost` and hands the untouched frame to CPU raster, matching `fault_tests.rs:142`. Reference-first: `.vendor/kwin-master/src/watchdog.cpp` is a systemd liveness deadline, not a per-reply watchdog; no smithay equivalent was found, so the pipe-child owner in `helper.rs` owns the deadline. Every edit is marked UNBUILT: git/cargo/builds were forbidden in this shared checkout, so compiling and running `fault_tests.rs` are deferred to the orchestrator.
