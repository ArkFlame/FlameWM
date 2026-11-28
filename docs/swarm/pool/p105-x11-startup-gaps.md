# Unit p105-x11-startup-gaps

STATUS: OPEN

Read-only static audit of the X11 WM no-start failure class. No crate edits, no git, no cargo, no build. Every row is a source read; none was reproduced on a live X server, so each finding is UNVERIFIED at runtime.

## Startup order (what runs before the X connection)

1. `flamewm-wm/src/main.rs:42` XSETTINGS claim (non-fatal, `main.rs:162-170`).
2. `flamewm-wm-x11/src/runtime/mod.rs:142-149` control bus (optional) then `Reactor::new()?` and `LinuxIntegrationRuntime::connect()?` / `start()` (fatal).
3. `runtime/mod.rs:155-175` provider and audio channels (fatal on error).
4. `runtime/mod.rs:259` `discover_catalog_once()?` (fatal).
5. `runtime/mod.rs:347` `run_with_hook` then `wm/event_loop.rs:54` `x11rb::connect(None)?`. X connection is established here, after steps 2 to 4.
6. `wm/event_loop.rs:66` `become_wm(...)?`, `event_loop.rs:71-79` `Wm::new(...)?`, `wm.scan_existing()?`.
7. `flamewm-wm/src/main.rs:52-55` any `Err` prints `flamewm: {error}` to stderr and calls `exit(1)`.

## Failure-class matrix

| Class | Source (file:line) | Today | Failure record with blast radius | Fault test |
| --- | --- | --- | --- | --- |
| No DISPLAY | `wm-x11/src/wm/event_loop.rs:54` (`x11rb::connect(None)?`) | Exit 1 via `flamewm-wm/src/main.rs:54`. Error is the raw x11rb text. | No. Only the raw error line. | None. |
| No DISPLAY, earlier stage | `runtime/mod.rs:145-149`, `:259` | Integration or catalog failure exits 1 before X is opened, so the operator sees a D-Bus or catalog error, not the X problem. | No. | None. |
| Another WM (`SubstructureRedirect`) | `wm-x11/src/wm/construct.rs:92-98` `second_wm_fault` (`ErrorKind::Access`), `:221-231` | Exit 1. Report lists `root-selection` with exit_code `Some(1)`. | Partial. Message is "another window manager is already running on this display" with no blast-radius clause (`construct.rs:95`). | Pure-function test: `wm/startup_fault_tests.rs` (u45). Not the real `become_wm` path. |
| RandR absent | `construct.rs:171-179`, `:194-196`, `:77-80` | Degrades to root geometry; runs on. | Yes, `extension` record. | Yes, `startup_fault_tests.rs:21-27`. |
| RandR query or selection failed | `construct.rs:81-87`, `:70-76` | Degrades; runs on. | Yes. | Yes, `startup_fault_tests.rs:30-60`. |
| Composite, Damage, Render, XFixes absent | `construct.rs:34-55`, `:200-209` | Degrades; runs on. | Yes, one per extension. | Pure function; the table is not exercised by a test read here. |
| XInput, SHAPE, XKB absent | `construct.rs:102-118`, `:210-219` | Degrades; runs on. | Yes. | Same as above. |
| Connection lost during `become_wm` | `construct.rs:161-167`, then `:171-219` | Keeps querying extensions on the dead connection. Each query error maps to "absent" via `is_ok_and` (`:205`, `:215`). Result: a false `extension absent` record for every optional extension after the real fault. Then exit 1. | Misleading. Real fault plus false absence records. | None. |
| Generic X11 error at `become_wm` (non-Access, e.g. BadValue) | `construct.rs:221-231` | Returns `Err`, report is empty, so nothing beyond the raw `flamewm: ...` line. Exit 1. | No. | None. |
| BadWindow during startup scan (client destroyed mid-scan) | `wm/manage.rs:24-33` (`manage(...)?`, `map_window(...)?`) reached from `event_loop.rs:79` (`scan_existing()?`) | Race guard `is_destroyed_window_race` (`wm/mod.rs:52`) is applied only in `handle_event` (`wm/events.rs:8-16`) and per-turn variant. `scan_existing` skips only attribute and geometry reply errors (`manage.rs:19-22`). A destroyed window in `manage` or `map_window` propagates and exits the WM at startup. | No. | None. |
| GL/GLX/EGL init fails (GPU chain) | `render-x11/src/accel/mod.rs:378-413` | Hardware stage then software stage, then CPU raster. Each stage emits `FLAMEWM-GPU-FALLBACK stage=... blast_radius=...` (`:351-355`). Runs on. | Yes. | Yes, `accel/fault_tests.rs` (u09c). |
| Chain only runs when GPU requested | `accel/mod.rs:429-431`; default `render-core/src/pixels.rs:44-45` (`Cpu`) | Default session never probes GL at startup, so a GL failure is silent by default. Expected by design, but the operator gets no startup line. | No record in default mode. | Not applicable. |
| llvmpipe only (software GL requested) | `accel/mod.rs:385-395` `is_software_renderer` (`:215-220`) | Records `hardware-gl` fail with `degraded:hardware-accel`, keeps the software device. Runs on. | Yes. | Partial: `accel/mod.rs:681-684` (classifier), `:737-741` (status shim). |
| Renderer open fails (no display, session, font) | `wm-x11/src/wm/construct.rs:269-271` `ExternalDecorationRenderer::open(...).ok()`; error from `render-x11/src/external_decoration.rs:101-116` | Silently drops the renderer. Decorations and cursors fall back, no record. | No. | None. |
| Startup shortcuts not grabbed | `wm-x11/src/runtime/mod.rs:439-447` | eprintln, runs on. | Message only, no blast radius. | None. |
| Session bus absent | `runtime/mod.rs:140-143` | Retried lazily; runs on. | Not checked here. | None read. |
| Durable failure file | `wm-x11/src` | None. Grep for `failure_record` and `OpenOptions` found only comments. The Wayland backend writes one (`wm-wayland/src/main.rs`, u36c). | X11 failures exist only on stderr. | n/a |
| Provider error mid-session (not startup) | `runtime/mod.rs:460-462` | Returns `Err`, exit 1, no record. | No. | None. |
| Dead helper code | `flamewm-x11-desktop/src/lib.rs:77-78` `X11Desktop::connect` | No non-test callers found (`rg 'X11Desktop::connect\('`). The live path is `from_connection` at `runtime/mod.rs:389-395`. | n/a | n/a |

## Fix specs (at most 3)

1. **One startup-failure record for every fatal exit.** In `flamewm-wm/src/main.rs:52-55` and `wm-x11/src/runtime/mod.rs` `run_session`, classify the error into a stage (`connect`, `integrations`, `catalog`, `become-wm`, `scan`). Print one `FLAMEWM-X11-STARTUP-FAIL stage=<s> cause=<c> blast_radius=<b>` line before `exit(1)`, and make the no-DISPLAY case say "DISPLAY unset or unreachable". Test: a pure classifier in `wm/startup_fault_tests.rs` with one case per stage. Gate note: exit code stays 1.
2. **Stop false absence records after a lost connection.** In `wm-x11/src/wm/construct.rs:161-219`, return early from `become_wm` when the change-window-attributes step produced `ConnectionError`, and map query errors to a distinct `Unknown` outcome instead of `absent` (`:201-205`, `:211-215`). Add the second-WM blast-radius clause at `:95`. Test: `startup_fault_tests.rs` case where the query returns an error must not emit an `absent` record.
3. **Survive a client destroyed during startup scan, and record a renderer loss.** In `wm-x11/src/wm/manage.rs:18-36`, treat `is_destroyed_window_race` errors from `manage` and `map_window` as skip-and-continue, not `?`. In `wm-x11/src/wm/construct.rs:269-271`, replace `.ok()` with a match that pushes a `renderer` `StartupFault` (blast radius: decorations and cursors use core frames). Test: pure predicate test for the skip, and the renderer fault record.

## Not done

- No runtime reproduction: no X server was started, no `DISPLAY` was unset in a live run, no screenshot was taken.
- Composite, Damage, Render, XFixes and XInput record paths were not read in their test files.
- Fix specs are not implemented; this file is docs only.

## Result

UNBUILT. No cargo, no tests, no X server run; no frame or runtime evidence. All three specs are implemented as source edits only.

- Spec 3: `wm/manage.rs` `scan_existing` skips `is_destroyed_window_race` errors from `manage`, `map_unframed`, `map_dock` and `map_window` and continues; other errors still `?`. `wm/construct.rs` `new` replaces `.ok()` with a match that pushes `renderer_fault` (stage `renderer`, blast radius core frames) and prints it.
- Spec 2: `wm/construct.rs` `become_wm` returns early when the change-window-attributes result is `ConnectionError`. Optional extension queries use tri-state `ExtensionQuery` (Present, Absent, Unknown); a failed query yields stage `extension-query`, never `extension`. Second-WM message gains the blast-radius clause (test updated).
- Spec 1: new `crates/flamewm-wm-x11/src/startup_failure.rs` (`X11StartupFailure`, `startup_stage`, `connect_cause`, `startup_failure_line`, `startup_failure_report`). Stages tagged in `wm/event_loop.rs` `run_with_hook` (connect, become-wm, scan) and `runtime/mod.rs` `run_session` (integrations, catalog). `crates/flamewm-wm/src/main.rs` prints one `FLAMEWM-X11-STARTUP-FAIL` line after the existing `flamewm:` line, exit code still 1. Untagged errors are `stage=unclassified`.
- Tests: `wm/startup_fault_tests.rs` gains cases for scan skip predicate, renderer record, Unknown vs Absent, lost-connection report, one classifier case per stage, no-DISPLAY cause, tagged and untagged errors. The lost-connection test checks the report only; the early return itself needs a live X connection to exercise.

Files: `crates/flamewm-wm-x11/src/startup_failure.rs` (new), `crates/flamewm-wm-x11/src/lib.rs`, `crates/flamewm-wm-x11/src/wm/manage.rs`, `crates/flamewm-wm-x11/src/wm/construct.rs`, `crates/flamewm-wm-x11/src/wm/event_loop.rs`, `crates/flamewm-wm-x11/src/runtime/mod.rs`, `crates/flamewm-wm-x11/src/wm/startup_fault_tests.rs`, `crates/flamewm-wm/src/main.rs`.

Open: `VERIFY-REAL-REGRESSIONS.md` not updated for the second-WM message change. Startup window-scan skip and renderer record need a live X capture. Other fatal stages (audio, provider registration after the first) are wrapped only where noted; `AudioHelper` and `UnixStream` errors still print as `unclassified`.
