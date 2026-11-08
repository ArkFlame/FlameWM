# Vela: feature inventory and comparison with FlameWM

Snapshot: 2026-11-08. Competitor: Vela, `vela-desktop/vela`, GPL-3.0-or-later, local read-only copy at `.vendor/vela-main/`.

## Method and limits

- Feature claims come from `.vendor/vela-main/README.md`, with line anchors below. Each feature is mapped to its owning source directory by name (for example `compositor/`, `shell/`, `lock/`, `bluetooth/`, `polkit/`, `updates/`). Directory file counts were read with `find`. Individual feature implementations were not audited file by file.
- Vela's own docs (`.vendor/vela-main/docs/*.md`, 35 files) back several README claims, including `docs/performance.md`, `docs/vulkan-device-loss.md`, `docs/polkit-agent.md`, `docs/bluetooth.md`, `docs/updates.md`. They are not all re-verified here.
- FlameWM status is taken from this repository's `README.md`, `docs/TODO.md`, `docs/WAYLAND_PLAN.md`, and source-keyword counts under `crates/`. Keyword hits prove that code exists. They do not prove the feature works. Those rows are marked `partial` or `unverified`.
- No runtime test of Vela was run. Vela's claims are reported as claimed, not as verified.

## 1. Vela feature inventory

Source path column: owning directory in `.vendor/vela-main/`, plus README line.

### 1.1 Window management and multitasking

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Snap layouts | Halves, quarters, six layouts (thirds, 2/3+1/3). Snap Assist. Snap groups shown and restored in the taskbar. | `shell/qml/SnapLayouts.qml`, `shell/src/` | L182-186 |
| Task View and virtual desktops | Super+Tab, drag windows between desktops, rename and reorder, Super+Ctrl+arrows with slide animation, persisted across sessions. | `shell/qml/TaskView.qml`, `shell/src/workspaces.cpp` | L187-190 |
| Alt+Tab with previews | Live previews including minimized windows. Quick Alt+Tab switches without showing the panel. | `shell/src/windowcapture.cpp` | L191-192 |
| Window animations | Open fade and rise, close shrink, minimize flies to taskbar button. | `compositor/src/scene/`, `shell/qml/` | L193-194 |
| Rounded corners and shadows | Drawn by the compositor. Off when maximized, snapped or fullscreen. | `compositor/src/scene/effects.c` | L195-196 |
| Vela title bar | Server-side decorations for Qt, KDE, X11 apps. Apps with client-side bars keep theirs. | `compositor/src/`, `shell/qml/` | L197-199 |
| Window menu and input | Invisible resize borders, Super+drag, Super+right-drag resize, Alt+Space menu with keyboard move/resize. | `compositor/src/`, `shell/` | L200-201 |

### 1.2 Shell

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Taskbar | Pinned and running apps, centered or left, drag reorder, jump lists with recent files, hover previews, StatusNotifierItem tray, one taskbar per monitor. | `shell/qml/`, `shell/src/` | L203-207 |
| App launcher | Instant search, pinned apps, keyboard navigation, Super+X power menu. | `shell/qml/` | L208 |
| Quick settings | Wi-Fi picker, Bluetooth, airplane, power mode, Night light, accessibility, brightness, volume, battery. Sound page with per-app volume and output. | `shell/qml/`, `brightness/`, `bluetooth/` | L209-214 |
| Media and volume keys | Work regardless of focus, with an indicator. Wheel over a taskbar button sets that app's volume. | `shell/src/` | L215-216 |
| Notifications | Actions, notification center with calendar (Super+N), Do Not Disturb. | `shell/qml/`, `shell/src/` | L217-218 |
| Clipboard history | Super+V. Text and images. Pinned items persist across reboot. | `shell/src/` | L219-220 |
| Snipping tool | Rectangle, window or full screen capture to clipboard and `Pictures/Screenshots`. | `capture/` | L221-222 |
| Desktop icons | Thumbnails, drag and drop, Properties, undo. Run (Super+R), Show desktop (Super+D). | `shell/qml/`, `shell/src/` | L223-224 |
| Light and dark, accent | Coordinated accent palette, applied to KDE and GTK apps. | `themes/`*, `shell/` | L225-226 |

`themes/` exists as a directory but has no source files (`find` returned 0 `.cpp/.h/.qml/CMakeLists.txt`). Theme plumbing is assumed to live in `shell/` and `common/`; not verified.

### 1.3 Files (`vela-files`, `explorer/`)

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Tabs, breadcrumbs, search | Subfolder search, Details and icon views remembered per folder, preview pane (images, text, video, PDF thumbnails). | `explorer/` (38 source files) | L228-230 |
| Background copy and move | Progress, conflict resolution, drag and drop everywhere. ZIP, 7z, TAR. Shortcuts, Favorites, Trash with restore. | `explorer/`, `common/` | L231-232 |
| Atomic file replacement | Write to hidden temp, rename on completion. Old content kept until new copy is safe. | `explorer/`, `common/` | L233-235 |
| Unmounted drives | udisks integration in This PC. Mount on double-click, eject from menu. | `explorer/`, `common/` | L236-237 |

### 1.4 Settings (`vela-settings`, `settings/`)

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Displays | Drag-and-drop layout, scale, resolution, refresh, rotation. "Keep these settings?" with compositor-owned revert on timeout or crash. | `settings/`, `compositor/` | L241-244 |
| Night light, VRR, tearing | Gamma LUT, sunset/sunrise schedule computed offline from timezone. | `compositor/`, `settings/` | L245 |
| Sound, Power, Clipboard, Input, Network | Mouse and touchpad gestures (3 and 4 finger). | `settings/` | L246-247 |
| Bluetooth | `vela-bluetooth` daemon pairs, connects, forgets. Answers PIN, passkey, confirmation prompts from the shell. | `bluetooth/` (12 files), `docs/bluetooth.md` | L248-250 |
| Updates | Installed and newest version, channels (Beta, Experimental), update in a terminal, background check with notification. | `updates/` (7 files), `docs/updates.md` | L251-253 |
| Personalization, Apps, Time, Accessibility | Wallpaper, colors, taskbar. Default apps. Layouts. Magnifier, colour filters, sticky keys. | `settings/`, `shell/` | L254-257 |

### 1.5 Graphics, gaming and input

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Own scene graph and Vulkan 1.4 renderer | No `wlr_scene`, no GL fallback. | `compositor/src/render/`, `compositor/src/scene/` | L260-261 |
| Live acrylic blur | Dual Kawase blur behind shell. Exposed via `ext-background-effect-v1`. | `compositor/src/` | L262-263 |
| Direct scanout, VRR, tearing, explicit sync | `linux-drm-syncobj-v1`, relative pointer, pointer constraints. | `compositor/src/` | L264-265 |
| Highest refresh rate pick | Picks max refresh at native resolution. | `compositor/src/` | L269-270 |
| Xwayland on demand | X11 apps 1:1 in monitor pixels, `Xft.dpi` follows scale. | `compositor/src/` | L271-273 |
| Accessibility | Magnifier (Super+Plus), grayscale and colour-blindness filters (Super+Ctrl+C), sticky keys. | `settings/`, `compositor/` | L274-276 |
| Touchpad gestures | Three or four fingers for Task View, desktop, switching. | `compositor/`, `settings/` | L277-278 |

### 1.6 Session reliability

| Feature | Vela detail | Source path | README |
| --- | --- | --- | --- |
| Crash recovery supervisor | Holds the Wayland socket. Restarts compositor on same socket, reconnects Qt/KDE apps. Locked state preserved. Three crashes in a minute ends session. | `session/` (1 file), `compositor/` | L281-284 |
| Shell crash recovery | Compositor restarts shell, windows stay open. | `shell/`, `compositor/` | L285-286 |
| Secure lock screen | `ext-session-lock-v1`. Screen stays black if locker dies. | `lock/` (4 files), `compositor/src/server.h` | L287-288 |
| GPU reset handling | Vulkan device loss: stop touching GPU, exit cleanly. Policy in `docs/vulkan-device-loss.md`. | `compositor/` | L289-290 |
| User Account Control | polkit agent dims and blurs desktop, one dialog, requests queue. Design in `docs/polkit-agent.md`. | `polkit/` (10 files) | L291-292 |
| Desktop service integration | GTK portals, wlr screen capture, KDE wallet, colour schemes, icon themes. | `compositor/`, `common/` | L293-294 |

### 1.7 Distribution and tooling

- Packaging for Arch, Fedora 44, Ubuntu 26.10 and others (`README.md` L291-346). Installer and packaging scripts are under `packaging/` (0 source files) and `scripts/` (`ci-check-package.sh`, `measure.sh`, `check-docs.py`).
- Build: CMake (`CMakeLists.txt`, 9.8K), Qt 6, C++20, wlroots 0.20 (README badges L15-22).
- Tests: `compositor/tests/`, `shell/tests/`, `tests/`. Coverage: `scripts/coverage-report.sh`.
- Vela publishes its performance method in `docs/performance.md` and `scripts/measure.sh`.

## 2. Vela advantages over FlameWM (as of 2026-11-08)

Ranked by user impact.

1. **Native Wayland session with its own compositor and Vulkan renderer.** Vela has a shipped compositor on Wayland. FlameWM's Wayland session has no successful native DRM login on record (`README.md`, Wayland known issues; `docs/WAYLAND_PLAN.md`).
2. **Crash recovery and session supervisor.** Compositor and shell restart with windows kept. FlameWM has a session supervisor (`README.md`, benchmark section, "Session mode") but no documented crash-restart policy comparable to Vela's.
3. **Secure lock screen on Wayland.** Vela uses `ext-session-lock-v1`. FlameWM lists "No session lock" as a Wayland known issue. FlameWM's X11 lock uses PAM (`crates/flamewm-lock/`).
4. **Display configuration with revert.** Vela has drag-and-drop layout, per-output scale, refresh, rotation, and "Keep these settings?" revert. FlameWM lists "no per-output layout or per-output scale" on Wayland.
5. **Fractional scaling that is pixel-exact.** Vela claims 1:1 buffer copy at 125 percent and 175 percent, with an automated bit-exact test (README L77-83). FlameWM has no measured fractional-scale evidence. It is `unverified`.
6. **Bluetooth pairing from the shell.** Vela has a dedicated daemon, prompts in the shell, quick settings page (`bluetooth/`). FlameWM has no Bluetooth implementation (1 keyword hit under `crates/`).
7. **Polkit authentication agent.** Vela ships one (`polkit/`). FlameWM has zero polkit hits.
8. **Accessibility.** Vela has magnifier, colour-blindness filters, sticky keys. FlameWM has zero magnifier hits.
9. **Task View, Alt+Tab with live previews, snap layouts with Snap Assist.** FlameWM has snap previews and keyboard workspaces but no Task View.
10. **Clipboard history with pinned items, notification centre, quick settings sheet.** Vela ships all three in one shell.
11. **Built-in update channel UI.** Vela's Settings shows installed and newest versions, channels and background check (`updates/`). FlameWM has no update UI.
12. **Direct scanout, VRR, tearing control, explicit sync** for fullscreen games (`compositor/`). FlameWM has no equivalent path on record.
13. **Tested design documentation.** Vela publishes 35 topic docs in `docs/`. FlameWM's `docs/` holds many dated historical reports, which are harder to use as current design reference.

## 3. What FlameWM does better than Vela

1. **Resource footprint of its own process set, measured.** FlameWM's 2026-10-30 idle snapshot: 23.8 MiB PSS (session mode) and 26.9 MiB PSS (direct mode), 57.4 and 58.9 MiB RSS, FlameWM-owned processes only, nested Xephyr at 1280x800, no apps open, one snapshot each. Source: `README.md` Benchmark, `docs/PERFORMANCE.md`.
   - Vela's figures are 17 MiB PSS (compositor) and 143 MiB PSS (shell), 0.1 percent CPU each, on a Ryzen 9 9950X with RX 9070 XT under CachyOS (`README.md` L423-440). Vela says these were "not independently repeated for this README update".
   - **Not comparable.** FlameWM excludes the X server, Vela's compositor is a Wayland process with a GPU renderer, and the sets differ. FlameWM's own README says its figures do not support a "fastest" or "lightest" claim, and the same applies to Vela's.
   - Vela's numbers are not a fair comparison basis. Both sides are `unmeasured` on the same machine in this session. Host load was about 28 on 2026-11-08, so no new idle measurement was taken.
2. **Smaller runtime stack in principle.** FlameWM says "no browser runtime" and compiles HTML/CSS shell UI to native rendering code (`README.md`). Vela uses Qt 6 and Qt Quick, C++20, and a 152-file shell. Installed size is `unmeasured`.
3. **X11 session in production.** FlameWM's X11 session is the default and documented (`README.md` Sessions). Vela is Wayland-only, with Xwayland for X apps.
4. **Assistant and AI integration (optional).** FlameWM has an opt-in ARK AI chat window with a Hermes proxy option (`README.md` FlameWM Assistant). Vela has no equivalent feature in its README.
5. **Desktop widgets.** FlameWM has clock, CPU, memory and media widgets (`crates/flamewm-widgets-core/`, `assets/screenshots/overview.webp`). Vela's README lists none.
6. **Weather popup and a task manager.** FlameWM has both as first-party apps. Vela's README lists neither.
7. **Licence.** FlameWM is LGPL-2.0-only with a pending MIT exception for `crates/flamewm-render-compiler`. Vela is GPL-3.0-or-later. Use case depends on the reader.
8. **Built-in terminal and finder.** FlameWM ships `flamewm-terminal` and `flamewm-finder`. Vela's README does not list a terminal app.

## 4. Feature matrix

Legend: `Y` shipped and documented, `N` absent, `partial` present with stated gaps, `unverified` code exists but no runtime evidence in this review. For Vela, `Y` means claimed in README, not runtime-verified.

| Area | Feature | Vela | FlameWM | Evidence (FlameWM) |
| --- | --- | --- | --- | --- |
| Session | Wayland native session | Y | partial | `README.md` Sessions; no successful DRM login on record |
| Session | X11 session | N (Xwayland only) | Y | `README.md` Sessions |
| Session | Crash recovery (compositor/shell restart) | Y | partial | `README.md` benchmark, session mode supervisor |
| Session | Secure lock screen | Y (ext-session-lock) | Y on X11 (PAM), N on Wayland | `crates/flamewm-lock/`; Wayland known issues |
| Session | Session lock on Wayland | Y | N | `README.md` Wayland known issues |
| Windows | Snap / tiling | Y (6 layouts + Snap Assist) | partial (snap previews) | `README.md` Features |
| Windows | Task View | Y | N | `crates/` keyword count `task view` = 1 |
| Windows | Alt+Tab with previews | Y | unverified | `crates/` alt-tab hits = 15, no preview evidence |
| Windows | Virtual desktops | Y (renamable, persisted) | partial (keyboard-switched workspaces) | `README.md` Features |
| Windows | Window animations | Y | unverified | no evidence |
| Windows | Rounded corners and shadows | Y | Y (glass) | `crates/flamewm-ui-core/src/glass.rs`, `assets/screenshots/start-menu.webp` |
| Shell | Taskbar with previews | Y | partial | `assets/screenshots/overview.webp` (no previews) |
| Shell | Start menu / launcher | Y | Y | `assets/screenshots/start-menu.webp` |
| Shell | Dock | N | partial (pill dock) | `docs/DOCK_DESIGN.md` status partial; `assets/screenshots/start-menu.webp` |
| Shell | Tray (StatusNotifierItem) | Y | partial | `crates/flamewm-tray/` |
| Shell | Quick settings sheet | Y | N | no evidence |
| Shell | Notification centre | Y | partial | `crates/` notification hits = 33; no centre evidence |
| Shell | Clipboard history | Y | partial | `crates/flamewm-clipboard/` |
| Shell | Screenshot tool with annotation | Y (rectangle, window, full) | Y (full, monitor, window, region, annotation, clipboard, ffmpeg record) | `docs/SCREENSHOT.md` |
| Shell | Desktop icons and context menu | Y | Y | `README.md` Features |
| Shell | Desktop widgets | N | Y (clock, CPU, memory, media) | `crates/flamewm-widgets-core/`, `assets/screenshots/overview.webp` |
| Shell | Wallpapers | Y | Y | `assets/` wallpapers |
| Shell | Light / dark theme | Y | partial (glass dark) | screenshots |
| Shell | AI assistant | N | Y (opt-in) | `docs/ASSISTANT.md` |
| Files | Explorer with tabs, breadcrumbs, preview pane | Y | partial (no tabs evidenced) | `assets/screenshots/overview.webp` |
| Files | Archive support | Y (ZIP, 7z, TAR) | Y (`flamewm-archiver`) | `crates/flamewm-archiver/` |
| Files | Trash with restore | Y | unverified | no evidence |
| Files | Atomic file replacement | Y | unverified | no evidence |
| Files | udisks drives | Y | partial (devices list in overview) | `assets/screenshots/overview.webp` |
| Settings | Settings app | Y | Y | `crates/flamewm-settings/` |
| Settings | Display layout and scale | Y (per-output, revert) | N on Wayland, partial on X11 | `README.md` Wayland known issues; `docs/TODO.md` Phase 6 |
| Settings | Night light | Y | partial | `crates/` night light hits = 4 |
| Settings | Bluetooth | Y | N | `crates/` bluetooth hits = 1 |
| Settings | Network (Wi-Fi) | Y | partial | `crates/` NetworkManager hits = 18; `docs/TODO.md` Phase 7 |
| Settings | Updates UI | Y | N | no evidence |
| Settings | Accessibility (magnifier, filters) | Y | N | 0 hits |
| Security | Polkit agent | Y | N | 0 hits |
| Graphics | Vulkan renderer | Y | N (X11 render path, WebRender-derived compiler) | `crates/flamewm-render-x11/`, `crates/flamewm-render-compiler/` |
| Graphics | Direct scanout / VRR / tearing | Y | N | no evidence |
| Graphics | Fractional scaling, pixel-exact | Y (claimed, test in README) | unverified | `crates/` fractional hits = 20, no runtime proof |
| Graphics | Xwayland | Y | Y (XWayland in Wayland session) | `crates/flamewm-wm-wayland/src/xwayland.rs` |
| Integration | Terminal app | N | Y | `crates/flamewm-terminal/` |
| Integration | Weather popup | N | Y (Open-Meteo, off by default) | `README.md` Features |
| Integration | Task manager | N | Y | `crates/flamewm-task-manager/` |
| Packaging | .deb / Arch package | Y (Arch, Fedora, Ubuntu) | Y (.deb and Arch scripts) | `scripts/build-deb`, `scripts/build-arch`, `README.md` Packaging |
| Packaging | Installed session accepted on real hardware | Y (claimed) | N (stated in `README.md`) | `README.md` Packaging |
| Quality | Automated docs and design checks | Y (`scripts/check-docs.py`, `check-qml-motion.py`) | Y (`scripts/verify-visual`, `tools/flamewm_visual_tests.py`) | `scripts/` listings |
| Licence | License | GPL-3.0-or-later | LGPL-2.0-only (pending exception) | `LICENSE`, `README.md` License |

## 5. Gaps FlameWM must close to match Vela on features

Top 10, ranked by user-visible gap. Detailed design comparison is in `DESIGN_COMPARISON.md`.

1. Native Wayland login on DRM/KMS. No successful login is on record.
2. Session lock on Wayland (ext-session-lock).
3. Per-output display layout, scale and revert.
4. Task View with window previews and virtual desktop management.
5. Alt+Tab with live window previews.
6. Quick settings sheet with Wi-Fi, Bluetooth, audio output per app.
7. Notification centre with history and Do Not Disturb.
8. Bluetooth daemon with in-shell pairing prompts.
9. Polkit authentication agent.
10. Accessibility: magnifier and colour-blindness filters.

## 6. Source map

- Competitor README: `.vendor/vela-main/README.md`
- Competitor docs: `.vendor/vela-main/docs/*.md`
- Competitor shell/compositor/explorer/settings/lock/bluetooth/polkit/updates trees: `.vendor/vela-main/{shell,compositor,explorer,settings,lock,bluetooth,polkit,updates}/`
- FlameWM status: `README.md`, `docs/TODO.md`, `docs/WAYLAND_PLAN.md`, `docs/PERFORMANCE.md`, `docs/DOCK_DESIGN.md`
