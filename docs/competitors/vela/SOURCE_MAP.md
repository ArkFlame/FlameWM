# Vela source map

Competitor: Vela (`.vendor/vela-main/`, version 0.2.0, GPL-3.0-or-later, CMake project `vela`).
Read-only reference. Nothing here is a dependency or runtime input for FlameWM.

Evidence rule: counts and anchors come from `wc`, `find`, `grep`, and direct reads of the
cited ranges. Files marked **(file-level)** were located but not read line by line; their
claims are unverified. No Vela build or runtime was executed.

## 1. Language, toolkit, build

| Area | Fact | Anchor |
|---|---|---|
| Language | C (compositor, tools, tests), C++20 (shell, settings, explorer, polkit, lock), QML (UI), Python 3.10+ (tests, scripts, packaging) | `CMakeLists.txt:4,11-13` |
| Build | CMake >= 3.22; `CMAKE_CXX_STANDARD 20`, `CMAKE_EXPORT_COMPILE_COMMANDS ON`; default `RelWithDebInfo` | `CMakeLists.txt:4,11-17` |
| Feature switches | `VELA_BUILD_COMPOSITOR`, `_SHELL`, `_TOOLS`, `_POLKIT`, `_CAPTURE_TOOLS`, `_GPU_TEST_TOOLS` (all ON except capture tools) | `CMakeLists.txt:22-27,60-63` |
| Compositor deps | wlroots 0.20 (`wlr_*` headers), Vulkan 1.4, shaderc (`glslc`) for shaders | `README.md` badges; `.github/workflows/build.yml:36,377` |
| UI deps | Qt 6 + LayerShellQt, polkit-agent-1 | `CMakeLists.txt:26,60` |
| Tests | CTest, Python `unittest`, gtest (`build.yml:36`) | `CMakeLists.txt:28-33`, `build.yml:36` |
| Packaging | Arch (`PKGBUILD`, `PKGBUILD.beta`), Fedora (`vela.spec`), Ubuntu (`build-depends`) | `packaging/` |

## 2. Size (code lines, counted by extension)

Counts: `find` by directory over `*.c *.h *.hpp *.cpp *.qml *.py *.sh *.cmake CMakeLists.txt *.glsl *.comp *.frag *.vert *.js *.mjs`, lines via `cat | wc -l`. Blank and comment lines included.

| Directory | Files | Code lines | Dominant language |
|---|---:|---:|---|
| `compositor/` | 176 | 37,708 | C (30,098 .c, 4,620 .h), Python tests 71 files |
| `shell/` | 154 | 31,909 | C++ 15,540, QML 10,641, headers 5,308 |
| `settings/` | 99 | 14,659 | QML 9,690, C++ 3,921 |
| `tests/` | 51 | 18,925 | Python 9,976, C++ 6,266 |
| `common/` | 71 | 10,126 | C++ 5,058, QML 2,517 |
| `explorer/` | 39 | 8,981 | QML 4,116, C++ 4,105 |
| `scripts/` | 34 | 5,912 | Python 5,418 |
| `bluetooth/` | 25 | 5,361 | C 4,294 |
| `tools/` | 15 | 5,413 | C 5,226 |
| `brightness/` | 31 | 5,042 | C++ 1,806, C 2,408 |
| `polkit/` | 17 | 3,204 | C++ 2,082, QML 567 |
| `packaging/` | 13 | 3,458 | Python 3,260 (packaging tests) |
| `report/` | 10 | 2,880 | Python 2,225 |
| `updates/` | 14 | 2,808 | C 2,065 |
| `lock/` | 5 | 1,081 | C++ 881 |
| `capture/` | 4 | 528 | C 411 |
| `session/` | 4 | 18 | config templates |
| `protocols/` | 0 | 0 | empty (see section 6) |
| **Total** | **741 source files** | **157,547 lines** | (incl. `docs/` 30 .md, 10,796 lines) |

`cloc` is not installed here, so these numbers are `wc` based.

Note: an earlier pass counted only C++/QML/headers and missed `*.c`. The table above
supersedes it.

## 3. Component map

| Component | Purpose | Key files (anchors) | Deps | IPC / protocols | Config |
|---|---|---|---|---|---|
| **compositor** | Wayland compositor on wlroots 0.20 with its own scene graph and Vulkan renderer | `compositor/src/main.c:1-124`; `server.c:609`; `view.c:1730`; `output.c:959`; `input.c:889`; `lock.c:874`; `command.c:874`; `xwayland.c:1129`; `scene/scene.h:1-40`; `scene/frame.c:674,1338` (damage ring); `render/vulkan.c` (file-level) | wlroots, Vulkan 1.4, pixman, pixman_region32 | Wayland xdg-shell, layer-shell, ext-foreign-toplevel, ext-image-capture-source, xdg-decoration, Xwayland, wlr-output-management, polkit | `vela.conf` via `config.c:17,174,202,212`; per-theme `*.conf` |
| **compositor/render + scene** | Own scene graph (not `wlr_scene`), damage tracking, frame clock, Vulkan passes | `scene/scene.c`, `scene/frame.c` (1,457 lines around damage ring), `scene/effects.c`, `scene/output_capture.c`, `render/renderer.c`, `render/pass.c`, `render/formats.c`, `render/allocator.c`, `render/texture.c`, `render/shaders/` | Vulkan 1.4, wlroots renderer hooks | dmabuf import, scanout | `direct-scanout=no` and `VELA_SCANOUT=0` (`scene/scene.h:173`) |
| **shell** | Panels, taskbar, start menu, Alt+Tab, notifications, quick settings (Qt Quick) | `shell/CMakeLists.txt`; `shell/src/*.cpp` (58 files, 15,540 C++ lines); `shell/qml/` (42 QML files) | Qt 6 Quick, LayerShellQt | Qt IPC to compositor (file-level) | `vela.conf`, `themes/*/` |
| **settings** | Settings app (Qt Quick), pages for display, sound, bluetooth, themes, updates | `settings/src/main.cpp`, `preferences.cpp`, `displays.cpp`, `datetime.cpp`, `defaultapps.cpp`, `keyboardlayouts.cpp`; `settings/qml/pages/` (39 pages) | Qt 6, `common/` | DBus / helpers (file-level) | Preferences via `preferences.cpp` (file-level) |
| **explorer** | File manager (Qt Quick) | `explorer/` (39 files, QML 4,116, C++ 4,105) | Qt 6 | file-level | (file-level) |
| **common** | Shared QML controls and theme singleton | `common/controls/Theme.qml` (tokens, motion), `Button.qml`, `Switch.qml`, `Slider.qml`, `ListItem.qml`, `Dialog.qml`, `Shadow.qml`, `Eased.qml`, `PanelMotion.qml`, `ColorFade.qml` | Qt 6 Quick | none | none |
| **lock** | Screen lock (C++) | `lock/` (5 files, C++ 881) | Qt 6 | PAM (file-level) | (file-level) |
| **polkit** | Authentication agent and prompt | `polkit/prompt/main.cpp`, `request.cpp`, `protocol.hpp`, `qml/Veil.qml` | polkit-agent-1 | polkit D-Bus | (file-level) |
| **brightness** | Backlight control, models and tests | `brightness/` (31 files, 5,042 lines) | udev / sysfs (file-level) | DBus (file-level) | (file-level) |
| **bluetooth** | BlueZ client, device UI | `bluetooth/` (25 files, 5,361 lines) | BlueZ | DBus (file-level) | (file-level) |
| **updates** | Update checks and state | `updates/` (14 files, 2,808 lines); `packaging/vela-update`, `vela-update-beta`, `vela-update-state`, `verify-attestation.py` | Python | GitHub releases (file-level) | `vela-update-state` |
| **report** | Crash / diagnostics report generation | `report/` (10 files, 2,880 lines) | Python | file-level | (file-level) |
| **capture** | Screen capture helper | `capture/` (4 files, 528 lines) | (file-level) | screencopy / image-capture (file-level) | (file-level) |
| **tools** | CLI test tools (`vela-screen-dump`, `vela-windows`, `vela-input`, `vela-panel`, `vela-pattern`, `vela-capture-probe`) | `tools/CMakeLists.txt`; `tools/vela-input.c`; `tools/shaders/slow.comp` | wlroots / Vulkan (file-level) | Wayland | none |
| **session** | Session target and env, `vela.desktop.in`, portal config | `session/vela-session.in`, `session/vela-session.target`, `session/vela-session-env`, `session/vela-portals.conf` | systemd user session | systemd | templates |
| **themes** | Theme and style data (`*.conf` + `*.png`) | `themes/vela.conf`, `themes/vela-floating.conf`, `themes/window-glass.conf`, `themes/{taskbar,startmenu,notifications,screenshots}/` | none | none | theme files |
| **tests** | Functional (Python), benchmarks, unit, GPU-tag tests | `tests/functional/test_*.py` (22 modules), `tests/functional/harness.py`, `run.py`; `tests/bluetooth/`, `tests/updates/` (gtest C++) | CTest, Python | Wayland / Xvfb | (file-level) |
| **scripts** | CI and developer checks | `scripts/check-docs.py`, `check-qml-motion.py`, `check-shared-values.py`, `sharpness-check.py`, `coverage-report.sh`, `run-nested.sh`, `private-session*.py/sh` | Python 3, sh | none | none |
| **docs** | Design and operation docs (30 .md) | `docs/renderer.md` (Vulkan-only design, §2 decisions), `docs/configuration.md` (vela.conf keys, theme files), `docs/testing.md`, `docs/visual-coherence.md` (§6.4 shared values), `docs/hardware.md`, `docs/vulkan-device-loss.md` | none | none | none |

## 4. Process architecture

- One compositor process: `compositor/src/main.c` builds `struct vela_server` in `server.c`,
  owns outputs, views, input, lock, capture, and launches children.
- Shell, settings, explorer, lock, polkit prompt: separate Qt processes (file-level for launch details).
- Child management: `compositor/src/supervisor.c` (416 lines) with `supervisor_children.c` (file-level).
  Restart and back-off details are not read.
- Brightness and Bluetooth run as helpers under `brightness/` and `bluetooth/` (file-level).
- Xwayland is in-process via `compositor/src/xwayland.c` (1,129 lines, file-level for
  its wlroots hooks).

## 5. Rendering stack and GPU acceleration

- **Graphics API: Vulkan only, version 1.4. No GLES or pixman fallback.**
  `docs/renderer.md` §2 (decisions table): "Vulkan only, version 1.4 ... no GLES/pixman fallback."
  Badge `README.md`: `Vulkan-1.4`.
- **Scene graph: own, not `wlr_scene`.** `docs/renderer.md` §2: "entirely our own, not `wlr_scene`."
  `compositor/src/scene/scene.h:1-40` describes the ownership rules.
- **Damage tracking**: `compositor/src/scene/frame.c` uses `wlr_damage_ring` for buffer-age
  history (`frame.c:674,1025,1338,1341,1457`). Output capture keeps its own `pixman_region32_t
  damage` (`scene/output_capture.c:82,227-245`).
- **Direct scanout**: on by default; `vela.conf` key `direct-scanout=no` forces compositing,
  and `VELA_SCANOUT=0` overrides it (`scene/scene.h:162-173`).
- **Blur**: always live, drawn each frame the backdrop changes (`docs/renderer.md` §2, §8).
  Acrylic tuning keys in `docs/configuration.md:97`: `blur-saturation`, `blur-strength`, `blur-tint`.
- **Shader files**: `compositor/src/render/shaders/` (file-level), and `tools/shaders/slow.comp`.
- **GPU selection**: no runtime backend switch found. Vulkan device is selected by
  wlroots' default path plus `vulkan.c` (file-level). Vulkan device-loss path: `docs/vulkan-device-loss.md`
  and `render/device_loss_policy.h` (file-level).
- **Frame pacing**: `compositor/src/frame_clock.c` (file-level) and `docs/renderer.md` §1 "draw as late
  as possible before the vblank."
- **Not used**: Qt Scene Graph RHI is the Qt side only. The compositor does not link Qt.
  Shell and settings render through Qt Quick (file-level for the RHI backend choice).

Config keys summarized from `docs/configuration.md` (section "Development and renderer diagnostics"
at line 287 and "Presentation, VRR and power" at line 255): `direct-scanout`, `vrr`, and
renderer diagnostics. The exact key list was not read in full.

## 6. Compositor approach

- Own `struct vela_view` list in recent-use order (`compositor/src/view.h:1-40`).
- Window kinds: Wayland (`view.c`) and X11 (`xwayland.c`) behind a common API.
- Input: `input.c` (889 lines), `keyboard.c` (398), `bindings.c` (330), `motion.c` (file-level).
- Output: `output.c` (959), `output_manager.c` (371), `output_config.c` (293), `output_preview.c` (293).
- Window management: `workspace.c` (618), `snap.c` (498), `focus.c` (381), `interact.c` (776).
- Decorations: `decoration.c` (566), drawn by the compositor (`docs/renderer.md` §9).
- Lock: `lock.c` (874), `lock_retry_policy.h` (file-level).
- Brightness and night light: `brightness.c` (681), `brightness_model.c`, `backlight_choice.c`,
  `sun.c` (file-level), `night_light_policy.h`.
- Security helpers: `path_security.c` (file-level), `polkit.c` (file-level).
- `protocols/` is empty in this snapshot, even though `CMakeLists.txt` and the wlroots
  dependency suggest it held custom protocol XML. Protocol XML must come from wlroots or
  generated sources (file-level, unverified).

## 7. Theming and tokens

- **Design tokens: `common/controls/Theme.qml`** (QML singleton). Motion: four durations
  (`fast`, `normal`, `slow`, `long`) and one cubic-bezier curve (`Theme.scale`, `Theme.decelerate`).
- **Compositor side of the same tokens**: `compositor/src/motion.h:1-25`. Four
  `VELA_MOTION_<TOKEN>_MS` defines and `VELA_DECELERATE_CURVE`, same values.
- **Drift check**: `scripts/check-shared-values.py:1-40` parses both sides and fails when
  they disagree (`docs/visual-coherence.md` §6.4). Also `scripts/check-qml-motion.py` (file-level).
- **Theme files**: `themes/*.conf`, `schema-version=1` is optional (`docs/configuration.md:96`).
  `inherits=<name>` chains up to 16 files (`docs/configuration.md:95`). A parent that
  inherits back is dropped.
- **Per-user themes**: `~/.config/vela/themes/`.
- **Parser**: `compositor/src/config.c` (288 lines) is a flat `key=value` reader with a
  realloc-grown entries array (`config.c:144,161`), accessors `vela_config_get`
  (`:202`) and `vela_config_flag` (`:212`). No sections, no types beyond string and bool.
- **Icons**: SVG per `docs/renderer.md` §9.6 (file-level for icon sources).

## 8. Settings model

- Settings app: `settings/src/preferences.cpp` and `preferences.h` (file-level for store
  format; likely a Qt-side store plus `vela.conf` writes).
- Pages: `settings/qml/pages/` (39 pages: Display, Sound, Bluetooth, Themes, Updates, Accessibility, Apps, and more).
- Undo: `settings/qml/UndoBar.qml`. Theme gallery: `ThemeGallery.qml`, `StyleGallery.qml`.
- Config file: `vela.conf` (flat text, see section 7).
- Translations: `settings/i18n/vela-settings_it.ts` (Qt Linguist).

## 9. Packaging

- Arch: `packaging/arch/PKGBUILD`, `PKGBUILD.beta`, plus `vela-qt-private-rebuild.hook`
  (a pacman hook that rebuilds Qt for private builds, file-level).
- Fedora: `packaging/fedora/vela.spec`.
- Ubuntu: `packaging/ubuntu/build-depends`.
- Update client: `packaging/vela-update`, `vela-update-beta`, `vela-update-state`,
  `verify-attestation.py` (attestation verification; file-level).
- Tests for packaging: `packaging/tests/` (11 Python test files, 3,260 lines).

## 10. Test strategy

- **CTest** registers packaging unittest, the functional modules, and the gtest suites. CMake
  turns each `tests/functional/test_*.py` into its own test (`CMakeLists.txt:84-95`).
  Functional tests need a compositor and a GPU; without one they report skip with code 77
  (`CMakeLists.txt:82-83`).
- **Functional modules**: 22 files (`tests/functional/test_*.py`): accessibility, animation, apps,
  brightness, capture, decorations, fullscreen_stack, scanout, screen_sharing, session, shell,
  windows, x11, x11_native, and more.
- **Unit tests**: gtest in `compositor/tests/*_test.cpp`, `common/tests/tst_*.qml` (QML tests),
  `settings/tests/*_test.cpp`, `tests/bluetooth/`, `tests/updates/`.
- **Visual**: `scripts/compare-text-rendering.py`, `scripts/sharpness-check.py`,
  `common/tests/TextRenderingScene.qml` (text pixel comparison, run under xvfb-run).
- **Benchmarks**: `compositor/tests/bench_scene_diff.py`, `docs/benchmarks/`.
- **Harness tests**: `tests/harness/test_harness.py`, `test_gpu_ci_guards.py`.

## 11. CI

Workflows in `.github/workflows/` (26 files total, notable ones):

| Workflow | Role | Anchor |
|---|---|---|
| `build.yml` | Full build, ctest (excluding packaging-update), text-rendering check under xvfb | `build.yml:36,146,155-156` |
| `ubuntu.yml`, `fedora.yml` | Distro build matrix (file-level) | |
| `gpu-nvidia.yml` | Self-hosted GPU runner on Vast.ai (`runs-on: vela-vast-...`), `vulkaninfo` gate, dmabuf extension check | `gpu-nvidia.yml:88,107,182,286-291` |
| `gpu-amd.yml`, `gpu-intel.yml` | Sampled GPU coverage (file-level) | |
| `sanitizers.yml` | ASan/UBSan lane (file-level) | |
| `static-analysis.yml`, `codeql.yml` | Static analysis (file-level) | |
| `coverage.yml`, `coverage-report.sh` | Coverage (`.codecov.yml`) | |
| `package-smoke.yml`, `distribution-metrics.yml` | Packaging and download metrics | |
| `scorecard.yml`, `osv-scanner.yml`, `zizmor.yml` | Supply-chain and workflow security | |
| `branch-freshness*.yml` | Branch currency checks | |
| `documentation.yml` | `scripts/check-docs.py` | |
| `labeler.yml` | PR labeling | |

Contributor gate: `.github/dco.yml` (DCO sign-off).

## 12. Per-component quality notes

Evidence level: **read** = inspected lines, **grep** = found by pattern, **file-level** = path and
size only.

| Component | Quality signal | Evidence |
|---|---|---|
| compositor | Clear ownership comments at the top of each header (`view.h:1-20`, `motion.h:1-20`). Pure policy headers split out (`lock_retry_policy.h`, `device_loss_policy.h`, `night_light_policy.h`, `cursor_policy.h`). Damage and capture carry explicit invariants (`output_capture.c:79-82`). Large files exist (`view.c` 1,730 lines, `xwayland.c` 1,129). | read (view.h, motion.h, output_capture.c, scene.h, frame.c grep) |
| shell / settings / explorer | QML-heavy with C++ glue. Tests exist per QML control (`common/tests/tst_*.qml`, `settings/tests/tst_*.qml`). | grep |
| scripts | Strong: drift checks and doc checks are unit-tested (`scripts/tests/test_*.py`, 9 files). | grep |
| packaging | Large (3,260 lines of Python) with its own test suite. Scope is distro packaging, out of FlameWM's scope. | file-level |
| config | Flat `key=value` with no sections and no types. Simple, but no validation beyond ranges (`docs/configuration.md:97`). | read (config.c grep, configuration.md) |
| tests | 22 functional modules and 48 test files. GPU tests skip cleanly without a GPU, which keeps CI runnable. | read (CMakeLists.txt:82-95) |
| CI | 26 workflows, supply-chain (scorecard, osv, zizmor) plus GPU-real runner. Broad. | file-level |
| protocols/ | Empty directory in this snapshot. A missing dir may signal an incomplete or generated tree. | file-level |

## 13. FlameWM owner map (for the ideas section)

Checked against `docs/ARCHITECTURE.md` §1 and the `crates/` list.

| Concern | FlameWM owner | Rust lines (`find crates/<c> -name '*.rs'`, `wc -l`; blank = not counted) |
|---|---|---:|
| X11 WM authority | `crates/flamewm-wm-x11`, `crates/flamewm-wm` | |
| Wayland compositor | `crates/flamewm-wm-wayland` (Smithay 0.7) | 23,017 |
| Renderer core / X11 / compiler | `crates/flamewm-render-core` / `flamewm-render-x11` / `flamewm-render-compiler` | 11,126 / 37,577 / 4,050 |
| Shell (V8) | `crates/flamewm-shell`, `flamewm-shell-core` | 37,295 (shell) |
| Settings model | `crates/flamewm-settings-core`, `flamewm-settings` | 2,639 (core) |
| Skin / theme tokens | `crates/flamewm-skin` | 2,214 |
| Platform services | `crates/flamewm-platform` | |
| Session | `crates/flamewm-session-core` | |
| Lock | `crates/flamewm-lock` | |
| Event sources | `crates/flamewm-reactor` | |
| Profiling | `crates/flamewm-profiler` | |

## 14. Ideas worth porting to FlameWM

Ranked, max 15. Verdict is critical: FlameWM is Rust, X11 + Smithay Wayland, low-memory,
with no GPU renderer dependency. Vela's GPU-first features do not fit and are rejected.
Owner and effort are from `docs/ARCHITECTURE.md` §1 and the crate list above.

| # | Idea | Vela anchor | FlameWM owner | Effort | Risk | Clear win? |
|---|---|---|---|---|---|---|
| 1 | Drift check for constants duplicated between Rust and HTML/CSS/JS (motion durations, curves) | `scripts/check-shared-values.py:1-40`, `compositor/src/motion.h:1-25`, `common/controls/Theme.qml` | `flamewm-skin` or `flamewm-render-compiler` | S | low | yes, only if FlameWM has duplicated values (verify with grep first) |
| 2 | Workflow security lint in CI (zizmor), plus OSV and scorecard | `.github/workflows/zizmor.yml`, `osv-scanner.yml`, `scorecard.yml` | repo CI (`.github/workflows/`) | S | low | yes. FlameWM has `ci.yml`, `arch.yml`, `dist.yml`, `release.yml` and no equivalent |
| 3 | Pure policy headers for retry and device-loss decisions, separate from I/O | `compositor/src/lock_retry_policy.h`, `compositor/src/render/device_loss_policy.h` (file-level) | `flamewm-lock`, `flamewm-platform` | S | low | conditional: FlameWM already splits `*-core` crates, so only extend the pattern where a retry policy is inline today |
| 4 | Path canonicalization and containment helper for user-supplied paths | `compositor/src/path_security.c` (file-level) | `flamewm-platform` | S | low | conditional: check existing path handling in `flamewm-platform` and `flamewm-explorer` first |
| 5 | Child supervisor with restart policy | `compositor/src/supervisor.c:1-416`, `supervisor_children.c` (file-level) | `flamewm-session-core` | M | medium | conditional: only if FlameWM children lack restart and back-off. Verify in `flamewm-session-core` first |
| 6 | Bounded inheritance chain for theme files (max 16, cycle-safe, schema-version) | `docs/configuration.md:95-96`, `compositor/src/config.c:174-212` | `flamewm-skin`, `flamewm-settings-core` | S-M | low | conditional: useful only if FlameWM skins lack inheritance |
| 7 | Sanitizer CI lane for the unsafe boundary | `.github/workflows/sanitizers.yml` (file-level) | repo CI; `docs/ARCHITECTURE.md` §6 unsafe boundary | S | low | conditional: Rust reduces the need. Run Miri or sanitizers only on unsafe-touching crates |
| 8 | DCO sign-off enforcement | `.github/dco.yml` | repo CI | S | low | conditional: depends on whether FlameWM requires sign-off |
| 9 | Buffer-age damage ring for compositor redraw | `compositor/src/scene/frame.c:674,1338,1341` | `flamewm-wm-wayland` (Smithay already tracks damage) | L | high | no. Smithay owns damage; a parallel ring adds memory and risk |
| 10 | Live acrylic blur behind panels | `docs/renderer.md` §8, `docs/configuration.md:97` | `flamewm-render-*` | L | high | no. GPU-bound, conflicts with low-memory, and FlameWM has no GPU path |
| 11 | Vulkan-only renderer, 1.4 minimum | `docs/renderer.md` §2 | `flamewm-render-*` | L | high | no. Drops the software fallback that FlameWM needs |
| 12 | Exit code 77 as a "skipped, no GPU" CTest result | `CMakeLists.txt:82-83` | repo test harness | S | medium | no. A skip is not runtime evidence, and FlameWM's rule requires fresh executed proof |
| 13 | Night light from sun position | `compositor/src/sun.c`, `night_light_policy.h` (file-level) | `flamewm-platform` | S | low | no. Low value for FlameWM's current scope |
| 14 | Distro update client with attestation | `packaging/vela-update`, `verify-attestation.py` | out of scope | L | high | no. Distro packaging is not FlameWM's product boundary |
| 15 | QML settings and file manager UI | `settings/qml/`, `explorer/qml/` | `flamewm-settings`, `flamewm-explorer` | L | high | no. FlameWM uses V8 HTML/CSS shell. Porting Qt would add a second UI runtime |

Top 5 by value: #2, #1, #3, #4, #6. #5 is the only medium-effort item and needs verification first.

## Skipped or not verified

## Skipped or not verified

- `cloc` is not installed; LOC comes from `wc`.
- File-level entries were not read; their claims stay unverified.
- No Vela build, ctest, or runtime run. Vela is a reference only.
- `protocols/` is empty here, so the wlroots protocol source is unverified.
- Risk: the `.vendor/` tree is read-only reference; no change was made to it.
