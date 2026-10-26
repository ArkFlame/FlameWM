## 0.1.2 — Separate-taskbar capsule alpha fix

- Separate-mode taskbar capsules now alpha-blend the blurred backdrop with the already-painted wallpaper instead of replacing it with opaque blurred pixels whenever opacity is nonzero. This fixes the apparent solid background at 1–5% and preserves the expected transparent 0% and opaque 100% endpoints.
- Premultiplied ARGB is used for the blurred pixmap and its cache key now includes opacity, so live settings changes invalidate the correct resource without stale backgrounds.
- Unified taskbar compositing, blur settings, capsule shapes and hover behaviors are unchanged.
- Source-level regression tests cover zero, low and full alpha and cache invalidation. Native Xephyr visual tests must still run on a graphical Linux host.

## Unreleased — Wayland tty session, terminal CPU/RAM, snap drag-out, translucency order

### Wayland (preliminary)
- Only `PauseDevice` for the scanout card quiesces the tty backend. logind answers every `ReleaseDevice` (including the input nodes libinput opens and closes while probing the seat) with `PauseDevice gone`; treating those as a session pause blanked the panel after the first frame and suspended input for good, which is the black screen with dead Caps Lock and no Ctrl+Alt+Fn seen under LightDM. libinput starts at init and keyboard LEDs follow the xkb state. Still unverified on hardware.

### Performance
- Text, visibility and font-weight writes on an absolutely positioned box that holds only text patch the retained layout in place instead of recomputing all of it. The terminal (a pool of ~6k such run boxes) drops from ~19% to ~3.4% of one core with a full-screen `top -d 0.1` (median of 3 runs, release builds, same workload).
- The per-node paint memo keeps no entry for hidden nodes and boxes the rest, and hidden subtrees skip style resolution when clip chains are built: terminal PSS 19.2 MB -> 15.3 MB.

### Terminal
- The PTY master is opened close-on-exec. Every child inherited it, so a closed or killed terminal never hung up its shell and programs such as `htop` kept running (and using CPU) as orphans.

### Window management
- Dragging a snapped or maximized window out under the outline (the default) restores its floating size at once, so the frame no longer keeps the snapped width while its chrome and state use the restored one (the "half cut" window).
- Pixmaps may exceed one root edge as long as their area stays within four root screens, and a requested app window starts at most screen-sized.

### Translucency
- A translucent window reads the windows stacked above it while they cover it (after a 250 ms quiet period, VisibilityNotify/Expose driven) into its cached root, so raising it shows those windows beneath it instead of the stale desktop. Only covered root pixels are read; the window is never hidden.

### Build
- Rust toolchain pin 1.85.1 -> 1.88.0 (`rust-toolchain.toml`, workspace `rust-version`, CI). The tree already uses let chains and its lockfile already needed 1.88, so the old pin could not build it.

## 0.1.1 — Preliminary Wayland session

### Wayland (preliminary)
- New `crates/flamewm-wm-wayland` (`flamewm-wayland`, smithay 0.7): native DRM/KMS backend behind the crate feature `drm`, plus a nested winit developer backend (`--backend=nested`) for running inside an existing session.
- Seat access is implemented directly against systemd-logind over D-Bus: `TakeControl`/`TakeDevice`, one `PauseDeviceComplete` per paused device only after the compositor quiesces, and `ResumeDevice` descriptor handling with a rebuild path. No libseat and no seatd.
- DRM/KMS through smithay `DrmCompositor` (GBM + EGL/GLES), libinput, Ctrl+Alt+F1..F12 VT switch and resume, and XWayland for the existing shell, desktop and launcher clients.
- Dirty-driven frame scheduling: a redraw request is drawn or deferred to the queued frame, never painted by an idle timer.
- Session wrapper `scripts/flamewm-wayland-session` resolves the backend explicitly (`--backend`, then `FLAMEWM_WAYLAND_BACKEND`, then `drm`), gates the native path on `--capabilities` reporting `drm_compiled=true`, logs to `~/.cache/flamewm/wayland-session.log` (0600, one `.1` rotation) and forwards signals without process-group kills.

### Packaging
- The `.deb` installs two session entries: `/usr/share/xsessions/flamewm.desktop`, whose name is now `FlameWM (X11)`, and `/usr/share/wayland-sessions/flamewm-wayland.desktop` (`FlameWM (Wayland)`, `Exec=/usr/libexec/flamewm/flamewm-wayland-session --backend=drm`).
- `Depends` adds the Wayland runtime set on top of the X11 stack: `libgbm1, libdrm2, libinput10, libudev1, libegl1, libgles2, xwayland`. No seat-library runtime package.
- `scripts/build-deb` builds with `--features flamewm-wm-wayland/drm`, preflights the libinput/gbm/libdrm/libudev development files plus `Xwayland`, and fails instead of packaging unless `flamewm-wayland --capabilities` prints `drm_compiled=true`.

### Diagnostics
- `flamewm-wayland --capabilities`: compile-time build facts only, no bus, no seat access.
- `flamewm-wayland --probe`: read-only advisory report; never gates startup or changes the exit code.
- Structured `FLAMEWM-WAYLAND-STARTUP stage=... status=... code=...` lines from the wrapper and the compositor.

### Documentation
- `README.md` gained a Wayland section with the two-session table, install/selection steps, diagnostics commands, limitations and the X11 recovery step; the X11 session is documented as the supported default.
- `docs/WAYLAND_PLAN.md` is linked from the README and is the implementation and status record. `docs/INSTALL.md`, `docs/PACKAGING.md`, `docs/ARCHITECTURE.md`, `docs/DEVELOPMENT.md`, `docs/VERIFICATION.md`, `docs/SCREENSHOT.md` and `docs/TODO.md` were updated; the statements that the package ships no Wayland session, that no Wayland backend exists, and that the `.deb` is X11-only were removed. `docs/WAYLAND_BACKEND_FEASIBILITY.md` is marked as the superseded pre-implementation analysis.

### Known unverified
- No successful native DRM/LightDM login has been validated on real hardware. A LightDM login reportedly reached a text cursor and returned to the greeter; that is under investigation and is not fixed. The Wayland session ships as preliminary and experimental.
- Unvalidated: VT switch and pause/resume on real hardware, session lock, screenshot/portal integration, native Wayland shell clients, output layout and per-output scale.
- Recovery path documented and expected: choose `FlameWM` (X11) at the login screen. The Wayland wrapper does not fall back by itself.

## Pass 26 — Test compilation, rounded taskbar paint, desktop text shadows
- Fix the exact E0425/E0433 errors: import `desktop_takes_focus_on_press` and `X11WindowRole` into the X11 event-loop test module.
- Synchronize floating taskbar rounded paint with the native XShape radius (correct physical/logical scaling); classic taskbar remains square. Avoid invalidating unchanged border radii.
- Add typed CSS `text-shadow` support, Xft-rendered shadow paint for desktop labels, and compiled RWR format v6 with backwards decoding of v2–v5.
- Correct `flamewm-render-inspect` to report the file version instead of hard-coded RWRB/2.
- Add focused Rust regressions and offline source-contract tests. Native Rust/Xephyr build not run in this environment.

## Pass 25 — Desktop Convert Cargo cache recovery
- Fail-closed release build invalidates stale desktop core/UI artifacts, and new `scripts/check-cargo` does the same for dev checks.
- Expanded conversion API guard and fresh-mtime overlay archive to prevent returning E0432/E0599/E0061 after an import.

# Changelog

## Pass 23 — Installed-system production defects (2026-10-19)

- Repair installed titlebar-control SVG resolution through the canonical package root and add a visible native fallback.
- Reproject weather to every taskbar, not only the primary panel.
- Preserve physical RandR primary output across monitor enumeration changes (with deterministic fallback).
- Display desktop launcher Name metadata, including locale values, independently from launchability.
- Register Super+E / Ctrl+Alt+E and Super+T / Ctrl+Alt+T through canonical X11 shortcut grabs.
- Add a read-only installed-system diagnostic for KDE Wallet service visibility, MIME defaults, and monitor topology.
- Full draggable monitor arrangement, live drag-ghost visibility, and KDE Wallet/MIME provider integration still require native work. See docs/FLAMEWM_PASS23_INSTALLED_PRODUCTION_STABILITY.md.


## Pass 22 — Debian release recovery (2026-10-19)

- One-command `./scripts/build-deb` now prevents stale `Architecture: any` .deb installation and verifies checksums and SDDM session.
- Source archive validated before Debian staging; incomplete package outputs cleaned on failure.
- Added regression coverage for successful native-metadata fixture builds and failed Cargo builds with stale artifacts.


## 0.1.0 — Pass 18 (source iteration, native verification pending)

- Desktop drag hides the stationary originals and shows a 50% opacity retained ghost until release.
- Desktop receives focused Delete key events; selected entries use Move to Trash confirmation.
- Convert wheel uses live Surfaces color/opacity/blur and a hover-only accent sector mask, no petal art.
- Ten external browser-only design concepts; Quiet radial (#04) integrated into native UI.


## 0.1.0

### Product and versioning

- Product renamed to FlameWM; the Rust WebRender branch name is no longer used for the product.

### Fixes

- NetworkManager SaveSecrets/DeleteSecrets and unknown SecretAgent members are answered instead of failing; empty secret hints are never sent.
- Keep the NetworkManager batch going when a single event fails.

### Refactors and performance

- Shared helpers (D-Bus message items, reconnect backoff, rounded corner geometry, X11 helper bodies, surface fill) replace duplicated code.
- Micro-optimizations: reused slot id buffers in shell projection, reused blur and backdrop scratch buffers, and fewer per-line allocations in render paths.

## 0.0.9 — 2026-09-24

### Platform and desktop integration

- Added typed settings/control transactions across platform services, D-Bus transport, and the Settings application.
- Added desktop wallpaper loading and portal-backed file selection for desktop and Settings workflows.
- Connected workspace synchronization and reactor presentation scenarios to event-driven desktop updates and verification.
- Improved snap-preview lifecycle and settings/shell projection behavior.

### Documentation

- Rewrote `README.md` for the 0.0.9 X11-only product: current crate layout (`crates/flamewm-wm` ships the `flamewm` binary on top of `crates/flamewm-wm-x11`), verified scripts, default shortcuts, configuration locations, and install/upgrade/uninstall steps (not tested).
- Benchmarks are pending measured runs via `tools/flamewm_perf_compare.py`; no benchmark figures are published.
- Recorded the license metadata mismatch (`LGPL-2.0-only` workspace versus `MIT` for `crates/flamewm-render-compiler`) as pending maintainer confirmation.

## 0.0.8 — 2026-09-09

### Interaction and performance convergence

- Desktop drag ghost follows pointer; canonical entries stay put until release commits existing grid/group semantics.
- Context menus use borderless rows with 16px Breeze icons; pointer menus flip upward and scroll when oversized.
- Rename is Linux no-clobber (`renameat2` NOREPLACE); grid position migrates; collisions fail without data loss.
- Start uses one canonical indexed StartModel; Power/session isolation explicit; stale icons never persist; submenu sizes from content.
- Status popups anchor from source node with End alignment and intrinsic sizing; network shows real AP rows; calendar dims adjacent days via foreground override.
- Sticky notes scoped per workspace (signal-driven), edit/move/resize/color/bold/delete, v1 loads / v2 writes.
- Startup/performance: scoped profiler phases with shared interval helper; packaged-first icon resolve (desktop icon ~4s -> ~0.3ms); shell startup self-wall isolated from blocking Control round-trips.
- Window chrome converges on panel material #1b1e20 with 38x31 controls, centered 16px glyphs, 20px app icon, measured title.


## 0.0.4 — 2026-09-05

### Product identity

- Promoted FlameWM Rust WebRender workspace identity to 0.0.4.
- Added canonical `FLAMEWM_VERSION` and pinned workspace `calloop` to 0.14.4.

## 0.0.2 — 2026-09-05

### Architecture

- Removed the external `/engine` design completely.
- Removed `icewm-rust` as an active dependency/runtime base.
- Established `crates/flamewm-wm` as FlameWM's independent Rust/X11 WM.
- Made `.vendor/` strictly reference-only; active source/path dependency checks reject vendor/engine reach-through.

### Build fixes

- Fixed the 0.0.1 invalid Cargo metadata (`rust-version = "0.0.1"`).
- FlameWM-owned product crates use Rust 1.85 / edition 2024. Promoted renderer/shell crates preserve edition 2021 during parity convergence instead of mixing an edition migration into the port.
- Fixed false `libfontconfig.so.1` doctor failures by using loader-backed library discovery.
- Removed the dead-code-failing IceWM Rust build path instead of suppressing its warnings.

### FlameWM Render 0.0.6 convergence

- Source-promoted the 0.0.6 renderer implementation into active FlameWM WebRender crates.
- Added ordered pointer-motion coalescing without crossing release/event boundaries.
- Added release-optimized interactive build contract (`opt-level=3`, Thin LTO, one codegen unit).
- Disabled unused X11 graphics-exposure traffic on cached image copy operations.
- Added cached transparency masks and rounded border strokes.
- Added press/motion/release interaction phases required by real drag/selection semantics.
- Promoted the 0.0.6 V8 HTML/CSS/assets/controller behavior as the visual baseline.
- Added `./verify-visual` against the approved 1350x641 Xephyr reference.

### Methodology

- Integrated the SourcePort post-mortem as a permanent parity-development rule.
- For visual/prototype parity: exact source owner -> mechanical translation/copy -> wire -> compile -> differential -> refactor.
- IceWM remains useful engineering reference material, but no longer defines FlameWM's runtime architecture or source ownership.
