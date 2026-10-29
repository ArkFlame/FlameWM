## 2026-10-29 — Widget grid

- The widget grid covers the whole primary work area: columns and rows are computed from the usable area, and cells stretch so the grid fills it edge to edge inside a 16px margin (8px gutters). Cells are close to square (target about 104px at 100%). The grid recomputes on resolution, scale, taskbar size or position, and output changes; widgets reflow proportionally.
- Drag and resize snap to any cell. Dropping a widget on a same-size widget swaps them; otherwise the dropped widget takes the nearest free cell. Lock Position is respected.
- While a widget is dragged or resized, every grid cell shows a faint outline. The outlines are removed on release and cost nothing while idle.
- The drag ghost is accent-tinted on a valid cell (including the nearest free cell after a collision) and error-tinted when nothing fits.
- Custom widgets can be 1x1, 2x1, 2x2, 3x2 or 4x2 cells. Other kinds keep their own allowed sizes; a size they do not allow snaps to the nearest one.
- Widgets resize only from their borders and corners. Dragging the left or top edge moves the anchor cell; the opposite edge stays fixed. Sizes snap to the nearest allowed span for the kind and are clamped inside the grid. The circular resize handle and the right-click "Size" rows are removed.
- Widget settings start at the system defaults. "Custom accent" and "Custom surface" are off by default; turning one on reveals its colour rows. Clock date size, corner radius and per-card backdrop blur are no longer options: cards use the global blur and a fixed radius. Existing settings migrate: an explicit accent turns Custom accent on, and `inheritSurface=0` turns Custom surface on.
- Known limit: the widget layout is kept for the primary output only. Widgets are not stored per output, and the grid is not laid out on secondary outputs.

## 2026-10-29 — Glass settings polish

- The taskbar material now inherits the shared surface glass by default (fill, opacity and blur match windows, widgets and menus). `taskbarMaterialMode=blur-only` and `custom-tint` still work when chosen explicitly; existing settings files keep their stored values.
- The "Glassify contrast zones" option is removed. Contrast zones are always glassified; `glassifyContrastZones` in an old settings file is ignored.
- Settings group zones use a 20% black overlay on glass. With Reduce Transparency they become a solid, darker shade of the window colour.

## 2026-10-29 — File wheel audio and video conversion

- The file wheel now offers audio (MP3, M4A, WAV, FLAC, OGG, Opus, AIFF, WMA) and video (MP4, MOV, MKV, WebM, AVI, WMV, GIF, MP3 export) conversions. They run through ffmpeg, which is detected at runtime; without it these entries are not offered. Outputs are saved beside the original under a free name, and the original is never replaced.
- Media conversion checks the ffmpeg encoders before offering a target, and the media time limit is now 30 minutes.
- The file wheel has three modes: Convert, Tools and Actions. Hold Shift, Ctrl or Alt while hovering to pick a mode, or click the hub to cycle. Tools offers image (resize, JPEG, strip metadata, rotate, grayscale via ImageMagick), audio and video presets (ffmpeg), ZIP and SHA-256. Actions offers Open, Reveal, Rename, Create Shortcut, Trash.
- Subtitle files (SRT and VTT) convert between each other.
- Tools also offers tar.gz archives, and Extract for ZIP and tar.gz files (into a new folder beside the archive; existing files are never replaced).
- Keyboard: with the wheel open, Tab cycles the mode and digits 1 to 8 pick a segment. Escape stops running conversions and tools.
- Wheel look: rounded wedge corners, a centre hub that shows the mode name, and an accent-colored glow around the highlighted segment.
- `appearance=light` in settings now selects the Light glass preset for menus and the wheel when no explicit glass values are set.
- Not yet included: drag and drop onto the wheel, a progress readout for media jobs, per-segment icons, sound on hover, and arrow-key selection (arrow keys are not delivered to the desktop as text).

## 2026-10-28 — File finder

- Add `flamewm-finder`, a glass file search window over a bounded filename index. Open it with Super+F or Ctrl+Alt+F; a second press or Escape closes it.
- The finder searches file names and paths, with filters `type:`, `in:` and `~`. Results show as a list, or as a grid of tiles with Left, Right, Up and Down; Ctrl+G switches a result list to tiles. The home view shows recent files as tiles.
- Type `>` or `/` for commands (keyboard help, duplicate files, index setup, home folder), and `@` for folder scopes.
- Super+F now opens the finder. WindowFullscreen's default moves to Super+Shift+F. Saved shortcut settings are kept as they are, so a saved fullscreen binding on Super+F keeps it and the finder then uses Ctrl+Alt+F only.

## 2026-10-28 — Desktop widgets, window margin, GPU rendering and one dist directory

- Add desktop widgets: time and date, CPU and memory ring gauges, a media player and a custom text widget. Add them from **Add Widgets** in the desktop menu (a gallery with a preview of each), drag to move, drag the corner grip to resize, and right-click to configure, lock or remove. Every widget has its own settings (elements on/off, sizes, colors, alignment, background, radius) and they are saved in `~/.local/state/desktop-widgets.state`.
- The custom widget takes `%username%`, `%hostname%`, `%time%`, `%date%`, `%weekday%`, `%cpu%`, `%memory%` and `%uptime%`, and MiniMessage-style tags such as `<white>`, `<red_background>`, `<#8cc8ff>`, `<bold>` and `<reset>`.
- Add the `windowMargin` setting (default 2 px, 0 to 32): snapped and maximized windows keep that gap from each other, the screen edges and the taskbar, and the snap preview shows the same rectangle.
- Add the `renderAcceleration` setting (CPU or GPU, under Appearance). CPU and GPU implement one pixel backend; GPU runs backdrop and menu blurs through OpenGL ES on an offscreen EGL context, switches live, and falls back to CPU when no GPU context can be made. A 1920x1080 blur takes 12 ms instead of 49 to 117 ms on an RTX 3050.
- Every packaging lane now publishes into `.target/dist`: `build-deb` (`.deb` and archives), `build-arch` (the pacman package) and the new `build-cpak` (cpak OCI image, plus a digest-pinned `cpak.json` with `--push`). `build-dist` runs the lanes that can run here, and `.github/workflows/dist.yml` builds all three and uploads them as one artifact.
- The cpak image is now compiled from source inside `archlinux:base-devel`, so it no longer depends on the host's glibc.

## 2026-10-28 — Idle Desktop memory and live backdrops on move

- Idle Desktop drops to 25.8 MiB PSS. Xft and its font libraries load on the first text draw, icon theme directories and index files are scanned on the first themed lookup, and identical node styles in compiled documents are shared.
- A moved window's translucent backdrop now updates as soon as it lands instead of on the next mouse move: the render loop no longer blocks while damage is latched.
- `tools/flamewm_visual_tests.py idle-memory` reports idle CPU, can open the apps (`--suite`), list each process's largest mappings (`--detail`) and run the binaries laid out like the package (`--packaged`).

## 2026-10-28 — Lower idle memory and a new Task Manager

- Cut idle session memory from 51.3 MiB to 39.2 MiB PSS with the default wallpaper, and to 31.1 MiB with a black solid one: the desktop no longer keeps a second copy of the wallpaper, release builds use fat LTO, the icon index stores its names in one buffer, and the WM, desktop and shell hand freed heap back once startup settles.
- Redesign the Task Manager as one page: htop-style processor meters and CPU, memory and network graphs on top, a Windows 11 style process table below, and the shared FlameWM context menu for End task and priority.
- Add `idle-memory` and `--solid-wallpaper` to `tools/flamewm_visual_tests.py`.
- Drop the minimize and two-screens screenshots from the README.

## 2026-10-28 — Two screens, visual checks and release packages

- Fix the release build, Clippy and three stale tests left by the light-outline pass.
- Count an output that scans out from a CRTC as a display even when its driver reports it disconnected. The second screen used to get no wallpaper, panel or work area.
- Lay out panel, popup and app-window backdrops per output from the new `_FLAMEWM_WALLPAPER_OUTPUTS` root property, so they match the desktop on every screen.
- Open the terminal's pty slave with `O_NOCTTY`; a terminal started as a session leader no longer dies with SIGHUP before its window appears.
- Crop window backdrops separably and recompute only the strips a move exposes. Terminal CPU while dragging with live contents drops from 53.9% to about 24% of a core.
- Add `tools/flamewm_visual_tests.py`: headless checks on one and two screens, run by the Arch CI job, plus a CPU and memory bench. Measured figures are in `docs/PERFORMANCE.md`.
- Add `.github/workflows/release.yml`: a `v<version>` tag builds the Arch package and the `.deb`, attaches them to a GitHub release and, with `AUR_SSH_PRIVATE_KEY` set, updates the `flamewm-git` AUR package.
- Move dated pass reports and handoff notes to `docs/history/`, drop the root links to `scripts/`, and remove the stale `MANIFEST.sha256`.
- Add README screenshots, issue and pull request templates, and starter tasks in `CONTRIBUTING.md`. The Wayland session is described by what was tested instead of being labelled preliminary.
## 2026-10-27 — Pass 17: Separated taskbar, workspace capsules, lower window contour, terminal TUI

- Hide the *root border* along with the root fill in `taskbarSeparate` mode; keep the root contour in unified mode and preserve the live `surfaceBorderColor`/`surfaceBorderOpacity` shared contract for individual capsules.
- Give `workspace-pager` the same 1px border geometry, surface fill, blur, and 8px radius as Start, task slots, tray and clock when separated; retain its individual square workspace buttons and active workspace highlighting. Clear the pager capsule in unified mode.
- Draw the missing bottom-left/bottom-right floating window contour with a compact bottom-only RGBA strip using the same `flamewm-ui-core::surface::inner_contour_coverage` and native renderer used by the titlebar. Alpha-composite only where coverage is nonzero; do not allocate full-frame buffers or introduce a new X11 window.
- Expand the terminal's build-time bounded run pool from 64 to 128 per row. The original 64-node limit was known to merge multi-colour block glyphs into a text run when overflowed; this specifically impacted dense OpenCode/htop TUI art. The tradeoff is approximately 6,144 additional precompiled hidden run nodes at the 96-row maximum, and potentially higher terminal memory. No new background threads or work per unchanged row.
- Add Rust regression tests for separated/unified border projection, bottom contour geometry and dense terminal runs. Static/integer-geometry checks are not a substitute for native Rust compilation, a terminal screenshot or live visual verification.

## 2026-10-27 — Pass 15: Round-glass contours, corner-shadow masking, Task Manager build merge

- Integrate the separate Command Code session's verified source edits for Task Manager: eliminate duplicate `priority` fields/initializer, dereference the borrowed resize height, and remove two unused imports. Update the outdated source-contract assertion that expected the non-compiling un-dereferenced resize call.
- Keep one Surfaces contour owner: share the configured `surfaceBorderColor`/`surfaceBorderOpacity` from `flamewm-ui-core` and `flamewm-ui-x11` with the panel root and all four separate-mode capsules; define their physical 1px CSS border widths so live color projection actually paints.
- Draw the X11 titlebar's curved upper outline with a reusable supersampled 1px inner-contour coverage function; preserve opaque titlebar pixels, configured border alpha, and frame clipping. Preserve partial rounded-ring coverage in the shared XRender stroke path when a clip cuts a surface.
- Anti-alias native minimize/maximize/close circle backplates using 4x4 sample coverage, with the SVG glyph still composited in front. No new renderer, compositor process or polling loop.
- Make X11 floating-frame shadow clipping follow the same frame radius as the window shape. Shade and retain only the right/bottom strips extended into exposed corner notches; invalidate the shadow shape on radius/style changes. Preserve the earlier square/zero-radius behavior and compact-shadow interfaces for callers/tests.
- Added focused Rust regression tests and 5 source/geometry preflight tests. 19 targeted Python checks passed; an unrelated historical visual-source suite has 3 pre-existing failures also reproduced on untouched Pass 14. Cargo and graphical X11 visual checks could not be run in this environment.
- See `docs/history/FLAMEWM_PASS15_ROUNDED_GLASS_AND_BUILD_MERGE_2026-10-27.md`.

## 2026-10-27 — Pass 14: Reference-backed CSS/scroll optimization and GPU architecture audit

- Audited the supplied COSMIC Epoch, CreamUI and Coconut source snapshots against the current FlameWM Rust render architecture; findings and GPU readiness gates are documented in `docs/history/FLAMEWM_REFERENCE_UI_GPU_RESEARCH_PASS14_2026-10-27.md`.
- Expand the build-time FlameWM CSS compiler with uniform `border: none`, `border: 0` and `border: 0px` resets; keep existing solid-border handling, strict rejection of unsupported complex borders and zero runtime dependencies. Align Task Manager preflight checks.
- Skip document layout/damage invalidation and offset-map insertion for clamped wheel input that does not change a scroll position. Preserve movement and clamp-on-content-shrink semantics.
- Wayland already uses Smithay OpenGL ES; X11 already uses server-side XRender where available. Do not add a redundant Vulkan/wgpu renderer or a misleading software toggle until equivalent backends and measured benefit exist.
- Source/package checks and targeted Python tests passed. New Rust tests are included but could not be executed without Cargo; native GPU, CPU/PSS and visual parity are **not** verified here.

## 2026-10-27 — Pass 13: Fix missing direct dependency in X11 surface adapter

- Add `flamewm-api` as a direct dependency of `flamewm-ui-x11`, matching `surface_style.rs` imports of `flamewm_api::settings::SettingsSnapshot` and its Rust unit-test import of `SettingValue`.
- Synchronize the `flamewm-ui-x11` entry in `Cargo.lock` so `--locked` builds accept the manifest update.
- Add a dependency regression preflight before the Xephyr release build, verifying both manifests and avoiding another expensive build ending at `E0433`.
- The four reported `unused_parens` warnings are already corrected in the supplied Pass 12 source; no renderer arithmetic or sort logic was changed. Shadow reference helpers remain unchanged to preserve differential regression coverage.
- Native Rust compilation and visual testing remain pending until a Rust/X11 workstation runs the build.

## 2026-10-27 — Pass 12: Fix Task Manager CSS border reset build failure

- Replace the unsupported `border:0` shorthand in `ui/task-manager/task-manager.css` with `border-width:0`. The FlameWM Render compiler accepts zero border width and preserves the button appearance, but requires three tokens for the `border` shorthand.
- Extend the existing Xephyr Task Manager CSS preflight to reject unsupported border shorthand values before the Cargo release build. Add regression cases for `border:0`, `border:none`, `border-width:none`, and valid equivalents.
- No compositor, process actions, renderer, or task-manager layout mechanics were changed. Native Cargo compilation still requires a Rust-enabled host.

## 2026-10-27 — Pass 11: Xephyr build/cache and Task Manager CSS repair

- Fix the native Task Manager stylesheet to use only CSS properties supported by the strict FlameWM Render compiler. Replace unsupported per-edge borders with existing border rendering and auto margins with flex layout; keep action IDs and process controls unchanged.
- Protect shared-surface crate builds against restored archive timestamps via content-hash-driven, package-scoped release cache invalidation for `flamewm-skin`, `flamewm-ui-core`, `flamewm-ui-x11` and `flamewm-task-manager`. This addresses old rlibs missing the source-present `surface`/`surface_style` modules and `ChromeTokens` fields.
- Run the CSS preflight and cache validation before the Xephyr release build. Add regression tests using fake Cargo metadata/clean without affecting user build artifacts.
- The earlier redundant-parentheses warnings were already resolved in Pass 10 and remain untouched. Native Rust compilation and graphical behavior still require workstation verification.

## 2026-10-27 — Native Task Manager / build consistency

- Added `flamewm-task-manager`, a native single-window htop-style process monitor with a fixed per-core CPU, memory, swap, load, threads and uptime header above a scrollable process table. Uses the same 1px glass surface contour and FlameWM accent as the other applications. No sidebar or permanent system-monitor daemon.
- Process sampling uses a 1-second off-UI-thread procfs worker with a bounded one-entry delivery channel and cached immutable command lines; PID/start-time identity is rechecked before explicit renice or SIGTERM actions. Right click offers fixed nice values, F7/F8 increment/decrement nice, and F9 requires termination confirmation. No `sudo` or command-shell interpretation.
- Integrated the task manager into the existing multi-call binary, source/installed package manifests, `.desktop` launchers, Xephyr nested session and Cargo.lock. Existing package regression fixtures now include the new desktop entry.
- Pass 9 already resolves `surface_stroke` compatibility re-export and `ChromeTokens.surface_border_rgb`/`surface_border_alpha` definitions from the previously reported Pass 8 failures; removed five related redundant-parenthesis compiler warnings.
- Live native build and visual verification are required on Linux: Rust toolchain and graphics display are unavailable in the packaging environment.

## Unreleased — shared surface materials and X Shape clipping, performance pass 9 (2026-10-27)

- Move the surface fill, 1px contour and RGB alpha-blend rules into the renderer-neutral `flamewm-ui-core::surface` owner. Retain re-exports from `context_menu` so existing consumers remain source-compatible.
- Add the single X11 `surface_style` adapter for RGBA conversion and border projection. Share it across Shell, Explorer, Terminal and Settings, and use the same blend in WM frame borders and native titlebar pixels. Preserve Explorer's development-only opacity override and Terminal's independent theme fill.
- Stream X Shape clipping directly into the rectangle merger instead of allocating an intermediate clipped span vector on every animated/clipped popup mask update; preserve the owned test helper.
- Focused Python suites: 181 passed. Three older `surface_icon_slide` assertions fail identically on the untouched Pass 8 archive. Independent 50,000 RGB blend and 1,200 clipped-mask pipeline comparisons pass. Rust compilation and native graphical profiling were unavailable.
- See `docs/history/PERFORMANCE_SHARED_SURFACES_PASS9_2026-10-27.md` for source ownership, impact, and verification limits.

## Unreleased — shared glass surfaces, Explorer edge and scroll allocation, pass 8 (2026-10-27)

- Add one configurable 1px inner Surfaces contour (`surfaceBorderColor`, `surfaceBorderOpacity`; default white/25%) shared by panel, popup roots, Explorer, Terminal, Settings, and native window chrome.
- Replace the hardcoded near-black X11 frame border with a contour-derived pixel and repaint existing frames when settings change. Source-level cause of Explorer right-edge seam identified, subject to live visual confirmation.
- Avoid a temporary heap vector on every Explorer row-scroll projection using a fixed-size stack window. Add Rust contour conformance checks. Python suite: 89 passed, 3 baseline-existing failures; Rust/native graphics not available. See `docs/history/FLAMEWM_PASS8_GLASS_CONTOUR_EXPLORER_SCROLL_2026-10-27.md`.

## Unreleased — X11 shadow row streaming, performance pass 7 (2026-10-27)

- Stream X Shape/shadow-shading row spans directly instead of allocating and clipping two full-height vectors per affected operation; preserve the existing owned span APIs for compatibility and differential tests.
- Keep mask geometry, rounded/tapered edges, frame clipping, RGBA/BGRX pixel arithmetic, focus behavior and X11 upload paths unchanged.
- Add Rust equivalence tests for degenerate/negative-coordinate geometries, packed masks and exact shaded bytes. Independent 6,000-layout / 2,577,763-row geometry check and 36 selected Python regressions pass. Rust compilation and live graphical CPU/PSS verification are pending. See `docs/history/PERFORMANCE_X11_SHADOW_ROW_STREAMING_PASS7_2026-10-27.md`.

## Unreleased — X11 selection-shadow allocation and GC optimization, pass 6 (2026-10-27)

- Reuse one temporary BGRX compositing buffer across all damaged shadow regions per desktop-selection update; release it at the end of the event.
- Create one X11 graphics context lazily per update instead of per affected window, with cleanup attempted even on upload errors.
- Resolve the visible compact shadow strip once per region instead of checking both strips per pixel, preserving the original lookup for nonstandard regions.
- Add Rust pixel-equivalence regression coverage and verify 1,018,240 random strip offsets and 36 Python tests. Rust compilation and graphical tests remain pending. See `docs/history/PERFORMANCE_X11_SELECTION_SHADOW_PASS6_2026-10-27.md`.

## Unreleased — X11 shadow shape-mask protocol optimization, pass 5 (2026-10-27)

- Merge vertically adjacent, identical X Shape rows into one rectangle while preserving the pixel-exact mask (including taper, rounded corner and frame interior exclusion). For a 1280×720 frame, 729 nonempty row rectangles become 32.
- Skip rebuilding/resending the X Shape mask when an existing shadow is reshaded without resizing; changes in focus, backdrop or window position do not change the window-local silhouette. New windows and width/height changes still update the mask.
- Add Rust shape-coverage and invalidation tests, plus independent geometry checks. Selected Python regressions pass; Rust compilation and live X11 results remain pending. See `docs/history/PERFORMANCE_X11_SHAPE_MASK_PASS5_2026-10-27.md`.

## Unreleased — X11 shadow CPU/allocation optimization, pass 4 (2026-10-27)

- Reuse wanted-frame, membership, geometry, output-layout and dragging buffers across X11 WM shadow reconciliation turns. Preserve sorting, hash contents and generation transitions while eliminating warm-state `Vec` allocator churn.
- Precompute the existing shadow opacity falloff at its 17 possible visible distances, replacing repeated floating-point work inside selection-shadow recomposition and initial visible-window shadow shading with integer lookups; retain the previous functions as visual-parity references.
- Add hash/gesture generation and per-pixel attenuation equivalence unit tests. Python regression suites and an independent float32 surrogate pass; Rust and graphical release verification remain pending. See `docs/history/PERFORMANCE_X11_SHADOW_CPU_PASS4_2026-10-27.md`.

## Unreleased — X11 desktop selection shadow-damage optimization, pass 3 (2026-10-27)

- Avoid recompositing unchanged selection-fill pixels under X11 floating-window shadows: when a rubber rectangle changes size without moving its top-left origin or colours, invalidate only the right/bottom edge bands, clipped to the visible shadow strips. Other movement and style changes retain the conservative path.
- Reuse one X11 graphics context for the affected regions of each shadow during selection updates, with cleanup attempted even when a region upload fails.
- Add Rust pixel-coverage and fallback-path regressions. An independent 2,400-case geometry differential passed. See `docs/history/PERFORMANCE_X11_SELECTION_DAMAGE_PASS3_2026-10-27.md`; Rust compilation and live visual proof remain pending.

## Unreleased — X11 interaction hot-path optimization, pass 2 (2026-10-27)

- Cache X11 ARGB visual discovery per display rather than repeating it during text-shadow and image uploads.
- Memoize fixed dock/ring XShape geometry with exact dimensions/parameters and invalidation after clipping/errors; leave dynamic coverage shapes unchanged.
- Avoid shadow generation/hashing when no floating/retained shadows exist, and reuse one successful X11 root-stacking lookup per batch of shadow refreshes.
- Add targeted Rust tests and document source-grounded savings, exclusions, and outstanding live validation in `docs/history/PERFORMANCE_X11_HOTPATH_PASS2_2026-10-27.md`.

## Unreleased — X11 shadow working-set optimization (2026-10-27)

- Retain only the right/bottom BGRX strips visible through the frame shadow's X Shape mask, instead of a full unshaded per-window snapshot. The full temporary readback is released after the initial upload; no additional persistent cache is introduced.
- Restrict live desktop-selection shadow recomposition and X11 image transfers to those visible bands rather than the old/new bounding region across the hidden frame interior. Pixel blending/shading logic remains shared with the original reference path.
- Shade only shaped, visible pixels when creating/refocusing window shadows. Keep the X11 pixmap, window shape, event/drag ordering, and Wayland paths unchanged.
- Add strip coverage, compact/full compositing equivalence, shading equivalence and memory-ratio unit tests. See `docs/history/PERFORMANCE_SHADOW_STRIPS_2026-10-27.md` for calculated gains and missing runtime verification.

## Unreleased — Wayland cursor, CPU and memory pass

Measured with `tools/bench-flamewm-session.py` (nested Wayland session, shell and desktop on
XWayland, terminal/explorer/settings opened, fixed 10 s XTEST workload; four runs each):
whole-session CPU median 8.2% -> 4.1%, allocated memory (heap + anonymous) 174 MB -> 134 MB.

### Wayland (preliminary)
- The pointer shows the cursor clients ask for: the surface set with `wl_pointer.set_cursor`
  (XWayland forwards every X11 cursor that way) or a named xcursor cursor, instead of a white
  square. Elements are `Kind::Cursor`, so the DRM compositor uses the hardware cursor plane. The
  theme is resolved like the X11 session (`flamewm-session-core::cursor_environment` and the saved
  `cursorTheme`) and exported before XWayland starts.
- Titlebar buttons are drawn above the titlebar (they were hidden under it).
- Titlebar fills keep stable element ids, so a pointer move no longer repaints every titlebar.
- `FLAMEWM_WAYLAND_DUMP_POINTER=<x>,<y>` places the pointer before a frame dump.

### Performance
- Translucent window backdrops are a 1/4-scale blurred canvas (about 0.5 MB at 1080p) instead of
  the decoded wallpaper (17 MB) or a full-size blurred root (8 MB); the wallpaper canvas is cached
  under `$XDG_CACHE_HOME/flamewm/backdrops` so later windows skip the decode. Terminal 38 -> 17 MB
  PSS and 5.1% -> 0.75% CPU under the workload; explorer 25 -> 10 MB PSS.
- A restack no longer re-crops, re-uploads and repaints identical backdrops.
- The X11 WM masks drag snapshots from its kept wallpaper canvas and no longer keeps a second,
  decoded copy of the wallpaper (17 MB for the default) after the first drag.
- The WM, desktop, shell, settings, explorer and terminal ship as one multi-call executable
  (`flamewm-apps`) with hard links, so their processes share the common code pages: those five
  session apps 98.9 -> 92.5 MB PSS, package executables about 47 -> 26 MB.

### Tools
- `tools/bench-flamewm-session.py`: ten-second CPU and memory benchmark of a whole desktop.
- Hardened benchmark output: only fresh private directories under the system temp or working directory; never recursively delete user-selected output. The benchmark now uses a private home/data/config/cache environment, filters child environment variables, rejects XTEST on the host DISPLAY or without a compositor-owned Xwayland child, and cleans up only its own process groups and temporary home. New security regression tests cover these cases.
- The terminal writes FlameWM profiler reports like the other apps.
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
- Full draggable monitor arrangement, live drag-ghost visibility, and KDE Wallet/MIME provider integration still require native work. See docs/history/FLAMEWM_PASS23_INSTALLED_PRODUCTION_STABILITY.md.


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

### FlameWM fullscreen, shortcut and startup-overlay fixes (2026-10-26)

- Require the live X server modifier mask to contain exactly Super+D or Ctrl+Alt+D for raw XInput show-desktop commands; ignore stale modifier cache and pointer button bits. X11 `Meta` bindings now alias Super, not Alt.
- Shape the X11 launch-feedback overlay from actual per-pixel icon alpha, preserving the image while removing opaque black padding on non-ARGB X11 windows; keep the indicator animation if a valid icon silhouette is unavailable.
- Track the original RandR root framebuffer dimensions as well as CRTC modes during fullscreen. Restore both safely after the game leaves fullscreen; grow before repositioning CRTCs and shrink afterward to support mixed-dimension changes.
- Refit fullscreen and maximized window geometry after host output changes (including windows on inactive Wayland workspaces), without adopting temporary fullscreen display geometry as the taskbar layout.
- Added focused unit tests for raw show-desktop modifiers, icon alpha shape, fullscreen geometry and resolution-tracker transitions.
