# Unit u36 — DRM backend failure fallback path audit

**Status: UNBUILT.** Static audit plus three source edits only. No `git`, `cargo`, build, test,
Python, `ctx_execute` or sleep loop was run in this shared-checkout unit; the orchestrator builds,
tests and commits after the batch lands.

Route evidence: workspace version `0.1.7` read directly from `Cargo.toml` (`scripts/version` invokes
Python, so it was not executed). Read `AGENTS.md`, `.opencode/HARNESS.md`, `.opencode/ROUTES.md`,
`.agents/README.md`, `.agents/CONTRACTS.md`, `.agents/JOBS.md`. Routed skills:
`flamewm-source-first` (current source, callers, contracts and error paths first) and
`flamewm-rust-systems` (ownership, failure boundaries and teardown). Shared checkout: no branch,
stash, reset, rebase or commit operation; only `main.rs` and this report were touched.

## Audit scope and method

Read `crates/flamewm-wm-wayland/src/main.rs` backend selection (`Backend`, `select_backend`, `run`,
`main`) and the tty init chain in `crates/flamewm-wm-wayland/src/drm/{init,device,mod}.rs`
(`libseat`/logind session, udev GPU scan, `DrmDevice`, `GbmDevice`, EGL/GBM, GLES renderer, connector
scan, libinput). Searched `libseat|LibSeat|udev|DrmDevice|GbmDevice|EGL` over
`crates/flamewm-wm-wayland/src`, plus `unwrap`/`expect`/`panic!` over the `drm/` module and logind.
The failure-record API is
`crates/flamewm-session-core/src/diagnostics/failure.rs`: `FailureRecord::at_now(stage,
FailureCode::SubsystemError, cause, BlastRadius::new(still_works, degraded), fallback_taken)` and
`append_record(path, &record)`; `flamewm_session_core::diagnostics` re-exports both.

Findings from the current source:

- `select_backend` (main.rs:177-184) maps only an explicit override or the compiled feature to
  `Native`/`Nested`; there is no headless mode and no detection of a usable seat before choosing
  native. `main` (main.rs:534-545) exits non-zero on any `Err` from `run`, so before this unit every
  native init failure ended the session rather than trying winit or a socket-only run.
- There are no production `unwrap`/`expect`/`panic!` in the DRM init chain. Hard failures are
  `?`-propagated `Box<dyn Error>` (GBM, EGL, renderer, udev backend, event-source registration) or
  explicit `return Err(...)` after a `StageStatus::Fail` line (no seat, inactive session, no GPU,
  no connector). "No input devices" is deliberately *soft*: `udev_assign_seat` failure only logs
  `Stage::Libinput status=fail` and returns, so it cannot strand the VT and is not a fallback case.
- All hard failures now land in `start_native`, which returns the distinct `BackendInitError`;
  `run` catches that type and tries nested winit, then a headless/minimal compositor that keeps the
  Wayland socket and event loop alive with `state.backend = None`. Each init-stage failure record is
  written at the point of the existing `StageStatus::Fail` line, so the record names the stage that
  actually failed (seat, drm or outputs), with `nested_winit` for the second backend.

## Rows — file:line, failure, now, change, risk

| file:line | Failure | Now | Change | Risk |
| --- | --- | --- | --- | --- |
| `crates/flamewm-wm-wayland/src/drm/init.rs:27-33`, `logind.rs` `LogindSession::new` | No seat / logind unavailable (no libseat service, no active session) | `stage_line(Seat, Fail, ...)` then `Err(error.into())`, propagated by `?` to `start_native`, then `main` exits 1. | `stage_line` (main.rs:412-427) now appends a `SubsystemError` `FailureRecord` with `stage=seat` and the seat/card blast radius; `start_native` returns `BackendInitError` and `run` falls through to nested. | Low — one file append on a path that already aborted; no change when the seat opens. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:43-50` | Session never becomes `Active` after `Session.Activate` (`reason=inactive_after_activate`) | Fail-closed `Err("logind session is not active; refusing to take DRM master")` and exit; deliberately avoids a black VT. | Same `stage=seat` failure record, then nested fallback instead of exit. | Low — fall-closed behavior kept; no DRM master is taken on an inactive session. |
| `crates/flamewm-wm-wayland/src/drm/device.rs:27-36,61,76-87`; `drm/init.rs:55` | No `/dev/dri` card on the seat, or every card fails `TakeDevice`/`DrmDevice::new` | `open_scanout_gpu` returns `no DRM device on seat`; `?` propagates a bare `Box<dyn Error>` with no per-step record. | `start_native` emits `stage=drm Fail` (record) and `BackendInitError`; `run` falls back. | Low — losing the scanout GPU degrades to nested/headless; record log is the only new side effect. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:72` | `GbmDevice::new(gpu_fd)` fails (no GBM driver / allocation device) | Bare `?`, no stage line of its own; `start_native` later prints one generic `stage=drm status=fail`. | Generic `stage=drm Fail` line now writes the typed failure record before the distinct error is returned. | Medium — GBM-specific detail is still only in the error text; stage granularity is `drm`, not a new GBM stage code. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:85-87` | `EGLDisplay::new` / `EGLContext::new` fails on the GBM device | Bare `?` through `unsafe` blocks, no record, then exit via `main`. | Same `stage=drm` record + `BackendInitError` + nested/headless fallback. | Medium — unsafe block unchanged; no new native handle escapes the fallback because `drm::init` failed before it built `Drm`. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:99-108` | GLES renderer / GL entry points fail to load | Bare `?`, exit via `main`. | Same `stage=drm` record and fallback. | Low — `dmabuf::init` has already run; nested init re-runs it against the winit renderer. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:110` | `UdevBackend::new(&seat)` fails (udev monitor / seat watch) | Bare `?`, exit via `main`; no libinput source reaches the loop. | Same `stage=drm` record and fallback. | Low — no udev source is installed on the failed path, so no stale watcher survives. |
| `crates/flamewm-wm-wayland/src/drm/init.rs:144-153` | `scan_connectors` fails, or no connector has a usable mode | `stage_line(Outputs, Fail, ...)` / `"no_connector"` then `return Err`, exit via `main`. | `stage_line` now appends `stage=outputs` record, then nested/headless fallback. | Low — an unplugged-panel machine now gets a socket-only compositor instead of a dead exit; no frames are claimed in headless mode. |
| `crates/flamewm-wm-wayland/src/drm/mod.rs:669-695`; `drm/init.rs:264-269` | No input devices: libinput `udev_assign_seat` fails or source registration fails | Soft: `stage_line(Libinput, Fail, ...)` and `return`, compositor keeps running without input. | Unchanged control flow; `stage_line` additionally appends a `fallback_taken=false` `stage=libinput` record so the degraded session is visible. | Low — no fallback triggered on this path by design: taking DRM master with `K_OFF` already set and no input would trap the user. |
| `crates/flamewm-wm-wayland/src/main.rs:936-978` (nested init) | No host display/winit surface: `init_from_attributes::<GlowRenderer>` or `insert_source(winit_source)` fails | `?` from `init_winit` reached `run` and `main`, exiting 1. | `start_nested_or_headless` (main.rs:1013-1050) records `stage=nested_winit`, clears a half-installed `state.backend`, reports skipped backend stages and keeps the socket-only event loop alive. | Medium — a client can connect but nothing paints; `render_frame` already no-ops with no backend (render/mod.rs:759-761), and XWayland/autostart behave as in any socket-only run. |
| `crates/flamewm-wm-wayland/src/main.rs:718-727,728` | All backends fail | `fill_backend_stages()` marked unreported `seat`/`drm`/`outputs` as `Ok` after any init, and `main` exited on error. | Fallthrough marks each skipped stage before `fill_backend_stages`, so the bit guard prevents a false `Ok`; only unexpected non-backend errors still reach `main`. | Low — stage lines for a successful backend are byte-identical; headless runs add explicit `skip` lines. |
| `crates/flamewm-session-core/src/diagnostics/failure.rs:69-135,185-193` | Failure record API | Existed, only used by the panic hook; compositor backend errors produced no records. | `record_backend_failure` (main.rs:386-407) builds `SubsystemError` records with stable `still_works`/`degraded` names and appends them to the same session log the panic hook uses. | Low — append-only, errors in the write are logged, never raised; repeated identical failures produce one record each (no dedupe). |

## Verification

Not run by this unit (see Status). The orchestrator should run the crate checks after the batch lands;
the three source edits are marked UNBUILT. Static read-back confirmed the edited regions and that
`main.rs` already imports `Error`, `env`, `PathBuf`, `getuid`, `session_log_path` and
`fallback_log_path`, so the helpers add no dependencies.

PARTIAL — the audit found no production `unwrap`, `expect` or `panic!` in the DRM/udev/libseat init
chain: every hard failure was an error return, and the real gap was that `main` exited instead of
trying another backend. Three edits now make each backend-stage failure write a typed `FailureRecord`
with stage and blast radius, return a distinct `BackendInitError`, and fall through native → winit
nested → headless/minimal, with the already-soft no-input path left unchanged. Verification and
commit belong to the orchestrator, so this slice is UNBUILT and reported as PARTIAL rather than PASS.
