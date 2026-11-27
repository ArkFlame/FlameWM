# u33 — Missing shared libraries: NEEDED audit and runtime-loading plan

**Status: UNBUILT.** No cargo build, test, or session launch was run in this unit; the worker
contract forbids `git`, `cargo`, python, builds and tests. The only check performed was
`bash -n scripts/flamewm-session-wrapper` (syntax only). Owned paths: this document and the
preflight section of `scripts/flamewm-session-wrapper`. No manifest, Rust source, or public API
was changed.

Owner evidence: `docs/DEPENDENCIES.md` (NEEDED table at lines 132-170, package table at lines
34-91, deliberately-not-declared at lines 93-120), `Cargo.toml` workspace entries, crate
manifests and `build.rs` files, `third_party/smithay/Cargo.toml`, `third_party/smithay/build.rs`,
`crates/flamewm-wm-wayland/src/probe.rs`, `crates/flamewm-render-x11/src/xft/backend.rs`, and the
lock PAM module. `.vendor/**` was not used as a dependency or runtime input.

## Audit method

A library is "NEEDED" when `readelf -d` shows it in a shipped binary (`docs/DEPENDENCIES.md`
lines 10-13). Some entries below are *dlopen*'d instead — they are in the brief's list because
they are runtime-loaded, but they are deliberately **not** in NEEDED, which is exactly why the
binary can start without them. That distinction drives the degrade column. Crate provenance was
read from `[dependencies.*]` / `[features]` blocks and `pkg_config::probe_library` calls; a
`links = "..."` key was not found in any first-party manifest, so native linkage comes from
transitive `-sys` crates and smithay's pkg-config probes.

## NEEDED / runtime-loading table

| Library | Which crate pulls it (manifest / build.rs evidence) | NEEDED or dlopen | Optional in function? | Cheapest degrade path |
| --- | --- | --- | --- | --- |
| libxkbcommon | `smithay` `[dependencies.xkbcommon]` 0.8, `features=["wayland"]` (`third_party/smithay/Cargo.toml:403`); `flamewm-lock` optional `xkbcommon` 0.8 behind the `wayland` feature (`crates/flamewm-lock/Cargo.toml:27,40-42`) | NEEDED in flamewm-wayland (`DEPENDENCIES.md:40`); ordinary link in lock's Wayland presenter | Compositor keyboard: no. Lock presenter: yes — default `x11` feature does not use it | Already optional at the crate boundary: build/run lock with default `x11`. Compositor has no cheap skip; wrapper preflight records `wayland_keyboard_layout` degradation and takes the X11/safe fallback |
| libinput | `smithay` `[dependencies.input]` 0.9 with `libinput_1_19` (`third_party/smithay/Cargo.toml:275-279`), enabled by `backend_libinput` (`:67`) via `flamewm-wm-wayland` feature `drm` (`crates/flamewm-wm-wayland/Cargo.toml:82`) | NEEDED when `drm` is compiled; probe lists `libinput.so.10` as `NativeOnly` (`probe.rs:79-83`) | Yes — only the tty/DRM backend | Cargo feature: build without `drm` and select the nested backend. If `drm` is already compiled, wrapper preflight falls back to the X11 session |
| libseat | `smithay` `[dependencies.libseat]` 0.2.3 optional (`third_party/smithay/Cargo.toml:288-291`) but **not enabled** by flamewm-wm-wayland ("No libseat", `crates/flamewm-wm-wayland/Cargo.toml:77`); smithay lists `"libseat"` in its docs feature (`:71`) | Not NEEDED; probe marks it `Informational` (`probe.rs:89-93`, `DEPENDENCIES.md:101`) | Yes, fully — seat comes from systemd-logind over D-Bus | No action: informational probe only. Wrapper records it with `degraded=none` if it ever appears |
| libudev | `smithay` `[dependencies.udev]` 0.9 (`third_party/smithay/Cargo.toml:340-342`) via `backend_udev` (`:73-75`) selected by `flamewm-wm-wayland` `drm` (`:84`) | NEEDED when `drm` is compiled; probe lists `libudev.so.1` as `NativeOnly` (`probe.rs:84-88`) | Yes — DRM backend device enumeration only | Cargo feature: drop `drm` / `backend_udev` and use nested backend; otherwise wrapper preflight selects X11/safe |
| libdrm | `smithay` `[dependencies.drm]` (`:244-246`) and `[dependencies.drm-ffi]` (`:248-250`) via `backend_drm`; `gbm` has `drm-support` (`:262-266`) | dlopen `libdrm.so.2` in the DRM backend (`probe.rs:74-78`, `DEPENDENCIES.md:49`) | Yes — DRM/KMS only | dlopen already present: smithay pulls `libloading` under `backend_egl` (`:55-57`), so a failed probe skips the backend without killing the process. Cargo feature off `drm`, or wrapper preflight fallback |
| libgbm | `smithay` `[dependencies.gbm]` 0.18 with `drm-support` (`third_party/smithay/Cargo.toml:262-266`) via `backend_gbm` (`:59-60`); `build.rs:78,104` uses `pkg_config::probe_library("gbm")` for capability probes | NEEDED in flamewm-wayland (`DEPENDENCIES.md:48`); probe `libgbm.so.1` is `NativeOnly` (`probe.rs:69-73`) | Yes at feature level; required once `drm` is compiled | Cargo feature: build without `backend_gbm`/`drm`. Wrapper preflight records `wayland_drm_backend+kms_presentation` and selects the next fallback; no dlopen wrapper exists today (build-time probe only) |
| libEGL | `smithay` `renderer_gl` → `gl_generator` + `backend_egl` (`third_party/smithay/Cargo.toml:55-57,117-120`); `renderer_glow` → `glow` (`:121-124`); winit `wayland-dlopen` (`:387-396`). X11 blur dlopens `libEGL.so.1` (`crates/flamewm-render-x11/src/accel/gles/loader.rs:31`) | dlopen for X11 GPU blur (`DEPENDENCIES.md:52`); probe requires it on both Wayland backends (`probe.rs:59-63`, `Always`) | Yes for X11 (CPU blur); no for the Wayland renderer probe | dlopen already in use: X11 falls back to CPU blur with no restart. Wayland needs a different backend — wrapper preflight selects X11/safe, or a `renderer_pixman`-only build (`smithay/Cargo.toml:129`) |
| libGL | `smithay` `renderer_gl` → `gl_generator` (`third_party/smithay/Cargo.toml:117-120`), `renderer_glow` → `glow` (`:121-124`); no first-party manifest declares `libGL` directly | dlopen'd through the EGL/libloading renderer path; no `libGL.so` NEEDED line in `DEPENDENCIES.md` | Yes — software/pixman renderer is an alternative | Cargo feature: swap `renderer_glow` for `renderer_pixman`/`renderer_test` (`smithay/Cargo.toml:121-130`). dlopen via the smithay `libloading` dependency; wrapper preflight fallback last |
| libpam | **No build-time dependency.** Lock PAM module is `dlopen`'d with `LIBRARY_NAMES = ["libpam.so.0", "libpam.so"]`; `DEPENDENCIES.md:44` records `flamewm-lock loads libpam.so.0` | dlopen, never NEEDED | Yes — lock authentication only; the X11 session still starts | dlopen already: `.ok()`-style load degrades to "cannot authenticate" without aborting. Wrapper preflight records `lock_authentication` and proceeds to safe mode |
| libasound | **No crate pulls it.** Repo-wide search finds no `asound` / `alsa` manifest entry or soname; it can only appear transitively under libpulse/Mesa, not as a FlameWM NEEDED entry | Neither — not declared by FlameWM | Yes / not applicable | No FlameWM action. If `ldd` reports it, follow the libpulse path (audio helper is isolated and skippable) and let the wrapper record it under the generic `unknown_component` blast radius |
| libpulse | `flamewm-audio-helper` → `flamewm-integrations-linux` feature `pulse-audio` (`crates/flamewm-audio-helper/Cargo.toml:17`) → `libpulse-binding` =2.30.1, optional (`crates/flamewm-integrations-linux/Cargo.toml:20,26`) | NEEDED in flamewm-audio-helper only (`DEPENDENCIES.md:43,146`) | Yes — the helper is spawned lazily and exits when idle; the WM never requires it | Cargo feature: turn off `pulse-audio` / do not spawn the helper. Cheapest at runtime: wrapper preflight records `degraded=audio_helper` and continues without audio |
| dbus | Workspace `dbus = "=0.9.12"` (`Cargo.toml:14`), used in about a dozen crates (control bus, lock, apps, session, wayland); optional in flamewm-wm-wayland (`crates/flamewm-wm-wayland/Cargo.toml:54,79`) | NEEDED in flamewm-apps, -session, -lock, -wayland (`DEPENDENCIES.md:42,158`) | No — core IPC transport (`DEPENDENCIES.md:158`) | None cheap: kept as NEEDED. Only the wrapper preflight fallback remains for a host so broken that even D-Bus cannot load |
| fontconfig | No first-party crate; dlopen `libfontconfig.so.1` / `libfontconfig.so` in `crates/flamewm-render-x11/src/xft/backend.rs:39-41`; `fc-list` also used as an external program (`DEPENDENCIES.md:71`) | dlopen on first text draw, not NEEDED | Yes — core X11 fonts still work (`DEPENDENCIES.md:47`) | dlopen already: `.ok()` load falls back to core X11 fonts. Wrapper preflight records `text_fonts`; no cargo feature needed |
| freetype | No first-party crate. Pulled transitively by libXft (Xft = fontconfig + FreeType + HarfBuzz, `crates/flamewm-render-x11/src/app/mod.rs:208`); compositor text uses pure-Rust `fontdue` (`crates/flamewm-wm-wayland/Cargo.toml:72`) | Transitive under libXft; no FlameWM NEEDED or dlopen line | Yes — same fallback as libXft/fontconfig | dlopen already via Xft; fall back to core X11 fonts or the pure-Rust `fontdue` path. Wrapper preflight records `text_fonts` |
| wayland-client | Workspace `wayland-client = "0.31"` (`Cargo.toml:29`); optional in `flamewm-lock` `wayland` feature (`crates/flamewm-lock/Cargo.toml:27,35-37`), dev-dependency of flamewm-wm-wayland (`:88`) and flamewm-screenshot (`crates/flamewm-screenshot/Cargo.toml:20`); smithay winit `wayland-dlopen` (`third_party/smithay/Cargo.toml:387-396`) | dlopen for the nested Wayland host backend (`DEPENDENCIES.md:67`) | Yes — nested host backend and Wayland lock presenter only | dlopen already via winit `wayland-dlopen`, so the nested backend can be skipped. Cargo feature: keep lock on default `x11`. Wrapper preflight records `nested_wayland+wayland_lock` and selects X11/safe |

## Implemented slice: wrapper preflight only

`scripts/flamewm-session-wrapper` gains four functions in the preflight section and replaces the
old `ldd`-then-`exit 78` block:

- `session_preflight_blast` — maps a soname to a `works|degraded` blast-radius pair, reusing the
  existing `chain_works` / `chain_degraded` vocabulary.
- `session_preflight_record` — emits one `FLAMEWM-SESSION-CHAIN ... blast_radius=works:...|degraded:...`
  line per missing library to stderr, the chain log, and `flamewm/session.log`.
- `session_preflight_missing_libs [binary]` — runs `ldd` on the compositor/session binary, extracts
  each `not found` soname (deduplicated), writes one failure record per library, writes a summary
  record with `next=safe`, then calls `run_safe_mode` instead of exiting.
- `preflight_libs BIN` — runs `ldd` on `BIN`, collects `not found` lines, and writes one failure
  record per missing soname to `flamewm/session.log` (same `FLAMEWM-SESSION-CHAIN ... status=fail
  ... blast_radius=works:...|degraded:...` format as `chain_record`, via `session_preflight_record`).
  It returns non-zero instead of exiting, so the chain skips the backend.

`preflight_libs "$SESSION"` is called before each backend launch in `chain_supervisor`: it gates the
x11 launch (non-zero selects safe), and records without gating safe mode, which stays the last
resort. The chain dispatch was moved below the preflight definitions so these helpers exist when the
supervisor runs. The direct-launch call site remains `session_preflight_missing_libs "$SESSION" || exit $?`. The helper takes a binary
argument so the Wayland wrapper can pass `FLAMEWM_WAYLAND_BINARY` in a later unit. No `cargo`
feature, dependency or `libloading` call was added in this slice; those rows are a plan only.

## Verification

- `bash -n scripts/flamewm-session-wrapper` — syntax only, as required by the brief: PASS. No shell
  execution, session launch, build or test was performed; runtime behavior stays unverified until
  the orchestrator runs it in a sandbox.

## Follow-ups

- Orchestrator: run the unbuilt acceptance script for wrapper fallbacks with an `ldd` stub that
  reports a missing lib; assert one `missing_lib` record per soname and that safe mode starts.
- A separate owner can port the same helper to `scripts/flamewm-wayland-session` with
  `FLAMEWM_WAYLAND_BINARY`; that wrapper was read only, not edited here.
- Rows proposing `dlopen` or cargo-feature changes are recommendations, not implemented work.
