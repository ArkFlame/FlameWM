# p100 backend fallback gaps

STATUS: OPEN

Read-only audit. No crate edits, no git, no cargo, no python, no sleep. Evidence is static source reads plus the indexed ripgrep results. Nothing was built or run, so every claim below is UNVERIFIED at runtime.

## Startup chain as read

- `crates/flamewm-wm-wayland/src/main.rs:864-889`: native start. On `Err` from `start_native`, `choose_native_fallback` (main.rs:289) picks `ExitWithCode(2)` when the failed stage is `seat`, otherwise `NestedWinit`.
- `main.rs:1186-1224`: `start_nested_or_headless`. A nested failure writes a record (`nested_winit`, fallback=true), then enters the headless branch. Headless stages are `Skip` with `headless_*` detail, and `stage_line` writes no record for `Skip`.
- `main.rs:1140-1183`: `BackendInitError`. Drm-feature build: `drm::init` failure goes through `stage_line(Stage::Drm, Fail)`. Non-drm build: explicit `record_backend_failure`.
- `main.rs:537-552`: `stage_line` writes a failure record for Fail on Seat, Drm, Outputs and Libinput. `fallback_taken` is true for Seat, Drm and Outputs.
- `probe.rs:58-95`: `--probe` checks EGL, GLESv2 (always), GBM, DRM, libinput, udev (native only). libseat is informational only. Seat is logind over D-Bus, not libseat.
- `render/gl_probe.rs:4`: `is_software_renderer` detects llvmpipe, softpipe and swrast. It is only a predicate in the files read; no caller was found that switches to a software path.
- `drm/init.rs:85,99`: `EGLDisplay::new` and `GlowRenderer::new` run inside `unsafe`, and their errors propagate with `?`. `drm/resume.rs:102` repeats EGL creation on resume.
- `logind.rs:373,542`: session activation is handled, with a 5 s budget and fail-closed on inactive.
- `main.rs:20-25`: there is no headless render backend by design. Headless means socket only.
- X11 WM: `crates/flamewm-render-x11/src/accel` has a hardware -> software -> CPU chain, covered by `accel/fault_tests.rs`. The X11 WM startup has `wm/startup_fault_tests.rs` (no extensions, no RandR, second WM).

## Gap table

Fallback today is the value the code takes when the condition occurs. Record means a failure record is appended with its blast radius. Test means a fault-injection test exists.

| # | Condition | Fallback today | Record with blast radius | Fault test | Verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | DRM device unavailable (native start) | `drm::init` fails, then nested winit, then headless socket-only | Yes, `stage_line(Drm, Fail)` writes it, fallback=true. Nested failure also writes `nested_winit`. | `backend_fallback_tests.rs:14,22,34,48` cover the pure mapping only, not `drm::init` or the runner | PARTIAL: mapping tested, runner not |
| 2 | Native start with no drm feature | Nested winit, then headless | Yes, explicit record at main.rs:1177 | `backend_fallback_tests.rs:48` | COVERED |
| 3 | No GL / EGL (native) | Same as row 1: `EGLDisplay::new` or `GlowRenderer::new` error becomes a Drm failure, so nested, then headless | Yes via the Drm stage | None injects an EGL failure | GAP |
| 4 | No GL / EGL (nested winit) | `init_winit` fails, record `nested_winit`, then headless with no renderer | Yes, fallback=true | `backend_fallback_tests.rs:34` is pure mapping only | PARTIAL |
| 5 | Software renderer (llvmpipe) on Wayland | No switch. `is_software_renderer` is a predicate with no caller found in the files read, so the compositor keeps GL on llvmpipe with no record | No | None | GAP (may be intended; unverified) |
| 6 | Headless entered after nested failure | Socket-only compositor, every render stage `Skip` | Only the `nested_winit` record. Entering headless itself is not recorded: `stage_line(Skip)` never writes a record | Mapping only (`:34`), no runner test | GAP |
| 7 | libseat missing | Not required. Session is logind, so no fallback needed | Not applicable (`probe.rs:13,90`) | `probe.rs:556` `libseat_is_never_required` | COVERED |
| 8 | Missing logind seat (native) | Exit code 2 (`EXIT_NO_SEAT`), no fallback | Record written by `stage_line(Seat, Fail)` with fallback_taken=TRUE, which contradicts the exit | `backend_fallback_tests.rs` imports `EXIT_NO_SEAT` but no test asserts the record's fallback flag | DEFECT: record says fallback when the process exits |
| 9 | Session inactive after activate (seat) | Fail closed, `reason=inactive_after_activate` | Yes, Seat Fail record | None found | PARTIAL |
| 10 | GPU hang or device lost mid-session | No device-lost or context-lost handler found. Render errors return through `?`, and the handling of an error from the frame path was not traced to the end | Not found | None | GAP (not fully traced) |
| 11 | DRM device removed or hot-unplugged | `drm/input.rs:44` handles input device removal only. No DRM device removal path found | Not found | None | GAP |
| 12 | Session paused (VT switch) and resumed | `logind.rs` pending list, `drm/resume.rs` rebuilds EGL | Not found | `state/tests.rs` covers queued requests only | PARTIAL |
| 13 | X11 WM: no GL, accel helper dies mid-session | hardware -> software -> CPU | Not verified in this audit | `accel/fault_tests.rs:71,105,142` | COVERED |
| 14 | X11 WM: no extensions, no RandR, second WM | Startup fault tests | Not verified in this audit | `wm/startup_fault_tests.rs` | COVERED |
| 15 | XWayland spawn failure | Compositor continues without X11 | `stage=xwayland status=fail` line (`main.rs` header); record not verified | None found | PARTIAL |

## Top gaps and fix specs (max 3)

### G1. Record the defect in row 8: seat exit must not claim fallback

- Where: `main.rs:537-552` (`stage_line`), and the caller at `main.rs:871-877`.
- Problem: `stage_line` sets `fallback_taken = matches!(stage, Seat | Drm | Outputs)`, so a seat failure records `fallback_taken=true`. `choose_native_fallback` then returns `ExitWithCode(2)`, so the process exits and no fallback happens.
- Fix: compute `fallback_taken` from the decision, not from the stage. Either pass the decision into the record write, or have the exit branch at main.rs:872 call `record_backend_failure("seat", cause, false)` after the stage line. Keep the stage line's record for the other stages unchanged.
- Test: add to `backend_fallback_tests.rs` a case that `NativeAvailability::from_failed_stage("seat")` maps to `ExitWithCode(EXIT_NO_SEAT)`, and that the record written for that path has `fallback_taken == false`. Use the injected path pattern from `each_failure_appends_a_record` (`:59`).
- Acceptance: `cargo test -p flamewm-wm-wayland -- backend_fallback_tests` and `--features drm` both exit 0 with the new case.

### G2. Record the headless entry (row 6)

- Where: `main.rs:1209-1223`, the `NextBackend::Headless` branch.
- Fix: before the loop at main.rs:1213, call `record_backend_failure("headless", "no rendering backend; socket only", true)`. Keep the existing `Skip` lines.
- Test: extend `backend_fallback_tests.rs:34` with an injected-path assertion that a `NestedFailed` outcome produces exactly one `headless` record line.
- Acceptance: the same two cargo commands exit 0.

### G3. Device-lost detection for mid-session GPU loss (row 10)

- Where: the frame path in `render/mod.rs` (the function that calls `backend.renderer()`, around lines 950-1000) and the main loop error handling in `main.rs`.
- Fix spec: classify render and swap errors whose text or `EGL_CONTEXT_LOST` / `EGL_BAD_ALLOC` marks a lost context as `context_lost`. On the first such error, write one failure record with stage `render_context_lost` and `fallback_taken=false`, then take the same path as the drm resume: rebuild the EGL display and renderer through `drm/resume.rs:102`. If the rebuild fails, end the session with a nonzero exit and the record already written. This is a spec only; the exact error type was not traced.
- Test: a fault-injection unit test that feeds a synthetic error through the classifier (pure function, no GPU) and asserts the record and the decision.
- Acceptance: classifier unit test exits 0. Runtime proof needs hardware and is UNVERIFIED here.

## Not done in this audit

- No runtime evidence. `backend_fallback_tests.rs` and the cargo suites are UNBUILT.
- `.vendor/**` was not consulted.
- Rows 5, 10, 11 and 15 need a deeper trace before a fix is scoped.

## Result

G1 (seat failure record): stage_failure_fallback(Stage::Seat) is false, so the logind-seat failure record now carries fallback_taken=false before the clean exit with EXIT_NO_SEAT. Test: seat_failure_takes_no_fallback in crates/flamewm-wm-wayland/src/backend_fallback_tests.rs.
G2 (headless entry): entering the headless step appends one record (stage="headless", fallback_taken=true) via append_headless_entry_record in crates/flamewm-wm-wayland/src/main.rs. Test: headless_entry_appends_a_record in backend_fallback_tests.rs.
G3: spec only, not touched.
Status: UNBUILT (no cargo/tests run per instruction).
