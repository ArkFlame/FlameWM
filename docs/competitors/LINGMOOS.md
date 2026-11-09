# LingmoOS (vendor copy) vs FlameWM

Scope: read-only static analysis of `.vendor/LingmoOS-main/`. No builds, no runtime evidence. Ideas only; no code is to be copied.

Evidence: FlameWM version `0.1.7` (`scripts/version`). Routes read: `.opencode/ROUTES.md`, `.opencode/HARNESS.md`, `docs/ARCHITECTURE.md`, `.agents/CONTRACTS.md`.

## Licence

- `.vendor/LingmoOS-main/LICENSE` is GPL-3.0 (GNU GPL v3, 29 June 2007). Most components carry the same header, e.g. `shell/bc/DockPanel/qml/main.qml` and `lib/libsysui/src/controls/Theme.qml:1-18`.
- Some subtrees carry other licences (`core32/LICENSES`, `shell32/*/LICENSES`, `bc/Launcher/LICENSE`). Treat each as separate.
- Policy: port design ideas and measured numbers as independent re-implementations. Do not copy QML, C++ or SVG assets. `.vendor/**` stays read-only and is never a dependency or runtime input (`AGENTS.md`).

## 1. Module map

Lingmo is a KDE-Plasma-derived stack (`shell32/lingmo-shell` = plasma-workspace, `shell32/lingmo-framework` = KF5 Plasma framework, `shell32/kscreen`). Its own shell apps live under `shell/bc/`.

| Component | Path | Notes |
|---|---|---|
| Dock | `shell/bc/DockPanel/` | Qt Quick window. Compositing-aware background, X11 strut via `src/xwindowinterface.cpp:162-175`, geometry in `src/mainwindow.cpp:178-232`. |
| Launcher | `shell/bc/Launcher/` | `qml/main.qml` (406 lines), `AllAppsView.qml`, `GridItemDelegate.qml`, `FolderView.qml`. C++ model in `src/applicationlistmodel.cpp`, `src/appmanager.cpp`, `src/iconthemeimageprovider.cpp`. |
| Status bar / control centre | `shell/bc/StatusBar/` | `src/statusbar.cpp`, `src/controlcenterdialog.cpp`, `src/systemtray/*` (StatusNotifierItem). QML `qml/DateTime.qml`, `qml/IconButton.qml`. |
| Desktop | `shell/bc/Desktop/` | `qml/Main.qml`, `desktop/desktopsettings.cpp`. |
| Settings / appearance | `shell/bc/Settings/src/appearance.cpp`, `shell/bc/StatusBar/src/appearance.cpp` | DBus proxies for dark mode and "dim wallpaper". |
| Design system (QML) | `lib/libsysui/src/controls/` | `Theme.qml` (tokens), `Units.qml` (spacing, icon sizes), `RoundedRect.qml`, plus `lingmo-style/*.qml` (Qt Quick Controls restyle). |
| Theme service | `lib/libsysui/src/thememanager.cpp` | Reads and watches dark mode, accent, font over session DBus. |
| Widget style (Qt Widgets) | `lib/libshell/ocean/` (Ocean, KDE Breeze-like) | `oceanui/oceanshadowhelper.cpp`, `oceanui/oceanblurhelper.h`, `oceanui/animations/*`. Not used by QML shell. |
| File manager | `fm/` | Out of scope for this pass beyond listing. |
| Shell32 (KDE-derived) | `shell32/lingmo-shell/`, `shell32/lingmo-framework/` | Plasma-style applets and containments. Large; not mapped in depth. |

FlameWM counterparts (current source):

| Area | FlameWM path |
|---|---|
| Design tokens + component CSS | `ui/shell/flamewm.css` (340 lines; `:root` tokens at lines 2-14), `ui/flamewm.css`, `ui/shared/`, `ui/base/` |
| Shell controller (V8 presentation) | `crates/flamewm-shell/src/` (~140 `.rs` files): `dock.rs`, `taskbar.rs`, `panel.rs`, `launcher_surface.rs`, `start.rs`, `tray.rs`, `notification_center.rs`, `quick_controls/`, `weather*.rs`, `icon_loader.rs` (814 lines), `surface_backdrop.rs` (108 lines), `async_projection.rs` (37 lines) |
| Blur | `crates/flamewm-image-core/src/blur.rs` (3-pass separable box blur, `MAX_BLUR_RADIUS = 64.0` at line 13, downscale above `DOWNSCALE_RADIUS` at line 16, reusable `BlurScratch` lines 46-74) |
| Architecture rules | `docs/ARCHITECTURE.md` section 5 (lines 82-86, V8 compiled at build time, no runtime HTML/CSS parser), section 7 (lines 92-101, shell consumes typed state and issues commands, never a second WM) |

## 2. Design tokens (Lingmo) vs FlameWM

Lingmo `lib/libsysui/src/controls/Theme.qml`:

- Surfaces, dark / light: background `#1C1C1D` / `#F3F4F9` (l.40), second background `#2C2C2D` / `#FFFFFF` (l.41), alternate `#3C3C3D` / `#F2F4F5` (l.42).
- Text `#FFFFFF` / `#323238`; disabled `#888888` / `#64646E` (l.44-45).
- Accent is runtime, from `ThemeManager.accentColor` (l.47). Fixed palette: blue, red, green, purple, pink, orange, grey, chosen by index (`thememanager.cpp:213-236`).
- Radii: small 8, medium 10, big 12, huge 14, window 11 (l.61-65).

Lingmo `Units.qml`:

- `smallSpacing` 6, `largeSpacing` 12 (l.54, l.62).
- Icon sizes 16 / 22 / 32 / 48 / 64 / 128 (l.39-44). `rowHeight = large * 0.95` (l.9).

Lingmo dock `shell/bc/DockPanel/qml/main.qml`:

- Radius = 0.3 x thickness (l.15), applied only when compositing and `Settings.style === 0` (l.36).
- Fill `#666666` dark / `#E6E6E6` light (l.44-45 of the rect), colour change animated 200 ms linear (l.46-50).
- Opacity 0.5 dark / 0.4 light when compositing, else 1 (l.39).
- Border 1 px / devicePixelRatio, alpha 0.3 dark / 0.2 light when compositing, 0.15 otherwise (l.20-33).
- Shadow and blur delegated to `LingmoUI.WindowShadow` strength 1 and `WindowBlur` `windowRadius = background.radius` (l.59-71). Blur region is handed to the compositor.

Lingmo launcher `shell/bc/Launcher/qml/main.qml`:

- Wallpaper: `Image` at `sourceSize` = screen (l.170-183), `FastBlur` radius 72, `cached: false` (l.185-188), `ColorOverlay` black at opacity 0.5 dimmed / 0.4 (l.190-197).
- Search field: radius = 0.2 x height (l.284), pill shapes elsewhere `radius: width / 2` (l.372), `GridItemDelegate.qml:214` `height / 2`.

Lingmo Ocean shadow presets (`lib/libshell/ocean/oceanui/oceanshadowhelper.cpp:44-70`), layered `(offset, blur radius, alpha)`:

- Small: (0,0,8,0.8), (0,-4,4,0.16), (0,-6,2,0.12).
- Medium: (0,0,20,0.24), (0,-4,8,0.32), (0,-6,4,0.01).
- Large: (0,0,28,0.20), (0,-8,16,0.24), (0,-13,6,0.16).
- X-large: (0,0,40,0.12), (0,-16,20,0.20), (0,-27,5,0.24).
- Tiles are rendered once and cached (`shadowTiles()`, lines 210-217).

FlameWM (`ui/shell/flamewm.css`):

- `--panel:#000000bf`, `--surface:#000000bf`, `--rim:#ffffff7f`, text `#f1f2f3`, muted `#aeb4bb` (line 3). Dark-only by default. Radii are `--radius-sm/md/lg/xl` (used at lines 36, 46, 52, 62, 74). Spacing `--space-xs/lg` (lines 24, 67).
- Window border 1 px `--rim`, radius `--radius-md` (line 36).
- Blur: no CSS `backdrop-filter` found (grep of `ui/`). Frosted surfaces are rendered by `flamewm-image-core::blur` (3 box passes, downscale for large radii).

Gap summary: Lingmo ships a light/dark pair for every token and a documented alpha ladder for dock vs. panel. FlameWM is dark-first with one set of alphas. Lingmo's values are not measurably better; they are a complete light/dark pairing.

## 3. Animation curves and durations

| Use | Lingmo | Path |
|---|---|---|
| Dock colour change | 200 ms, Linear | `DockPanel/qml/main.qml:46-50` |
| Launcher search field x | 200 ms (Behavior) | `Launcher/qml/main.qml:255-257` |
| All-apps paging (contentX) | 300 ms, InOutQuad | `Launcher/qml/AllAppsView.qml:60-64`, `:111-114` |
| Control-centre colour | 200 ms | `StatusBar/qml/DateTime.qml:99-100` |
| Launch/open transition (scale OutCubic 180 ms, opacity 250 ms, blur 300 ms) | **commented out** | `Launcher/qml/main.qml:107-154` |
| Ocean widget engines | `duration()` passed through per widget | `lib/libshell/ocean/oceanui/animations/*.cpp` |

Observation: Lingmo's launcher open/close motion is disabled in source. Do not cite it as shipped behaviour.

FlameWM: motion values live in `ui/shell/flamewm.css` and the Rust controllers. Not inventoried in this pass; check `grep -n transition` before porting any number.

## 4. Dock, panel, launcher, control centre, notifications

- Dock: geometry is computed from `availableGeometry` and `edgeMargins`. Margins are halved when compositing (`DockPanel/src/mainwindow.cpp:185`). The dock is a reserved strut (`xwindowinterface.cpp:162-175`). In FlameWM the WM owns struts and workspace area, so the strut logic must not move into the shell.
- Launcher: 2 layouts (grid, folder) and a paged all-apps view. Drag starts after a timer (`GridItemDelegate.qml:65-74`). Dragged item shows 0.5 opacity (`:107`). Empty-state text is not present in the launcher files read.
- Control centre: a Qt Quick popup with `windowRadius = _background.radius`, `radius: bigRadius * 1.5` (`DateTime.qml:84-90`), opacity 0.6 / 0.8 by mode (`:92`). Icons async (`qml/IconButton.qml:74`, `asynchronous: true`).
- Status tray: StatusNotifierItem host (`StatusBar/src/systemtray/statusnotifieritemhost.cpp`). Not read in depth.
- Notifications: `StatusBar/src/notifications.cpp`, `core32/libnotificationmanager`. Not read in depth; no visual tokens extracted.

## 5. Icon theme handling

`shell/bc/Launcher/src/iconthemeimageprovider.cpp`:

- Requested size clamped to >= 1 px (l.31-36).
- Paths starting with `/` load from file and scale (l.43-44).
- Otherwise `QIcon::fromTheme(id)` with fallback to `application-x-desktop` (l.47-49) and `icon.pixmap(size)` (l.50).

Useful pattern: one provider, one fallback name, size normalised. FlameWM's `crates/flamewm-shell/src/icon_loader.rs` (814 lines) already owns this; compare before any change.

## 6. Dark / light switching and accent

- `lib/libsysui/src/thememanager.cpp`:
  - Initial read over DBus: `isDarkMode`, `accentColor`, `systemFontPointSize`, `systemFont` (l.59-70).
  - Live updates by DBus signals `darkModeChanged` and `accentColorChanged` (l.82-84), no polling.
  - Accent index to colour switch in `setAccentColor` (l.206-236), with an early return when the index is unchanged (l.208).
- Wallpaper dimming follows dark mode: `darkModeDimsWallpaer` property (`StatusBar/src/appearance.cpp:58,71`).
- Dark / light colours are chosen in QML with `darkMode ? a : b` ternaries across the tree.

FlameWM: settings come from `flamewm-settings` through the typed snapshot path (`crates/flamewm-shell/src/settings_projection/`). Check whether dark / accent changes arrive as pushes or are re-read on each open before adopting the push model.

## 7. Responsiveness and reliability techniques

- Launcher AllApps: `cacheBuffer: control.width * control.count` (`AllAppsView.qml:55`). This keeps every page instantiated. Memory cost grows with app count. REJECT.
- Wallpaper blur: `FastBlur` with `cached: false` (`Launcher/qml/main.qml:185-188`) means the blur is recomputed each frame it is needed. Cost is high. The launcher clears its pixmap cache on wallpaper change (`:174-180`). REJECT this design.
- Ocean shadow tiles are built once and reused (`oceanshadowhelper.cpp:210-217`). Idea is sound.
- Image loads: `asynchronous: false` and `cache: false` on the wallpaper (`main.qml:174-175`). Synchronous decode on the UI thread. REJECT.
- Icons: `asynchronous: true` on status buttons (`StatusBar/qml/IconButton.qml:74`).
- DBus: `asyncCall` for uninstall (`Launcher/src/appmanager.cpp:35`). Other calls are synchronous (`iface.property(...)` in `thememanager.cpp:63-70`).
- Blur and shadow are handed to the compositor through `WindowBlur`/`WindowShadow` (`DockPanel/qml/main.qml:59-71`). No CPU blur in the dock path.

FlameWM already has: a 3-pass box blur with reusable scratch and release after emission (`crates/flamewm-image-core/src/blur.rs:46-74`), async projection (`async_projection.rs`), and a compiled (not runtime-parsed) UI (`docs/ARCHITECTURE.md:86`). These are ahead of Lingmo on memory and parse cost.

## 8. Ranked ideas

Max eight. Verdicts respect FlameWM rules: typed facades, UI never WM authority, no perf/memory regression, blur and surface look unchanged.

1. **Light/dark token pairs for every surface and text token.** What: define `--panel-dark`/`--panel-light` (and text, rim) in `ui/shell/flamewm.css :root` and switch under a `data-theme` attribute. Why: Lingmo pairs every token (`Theme.qml:40-45`). FlameWM is dark-only today. Files: `ui/shell/flamewm.css:2-14`, `ui/flamewm.css`, `ui/shared/`. Cost: CSS only, no runtime cost. Verdict: **ADAPT**. Keep current dark values as the default so the look is unchanged; add light only if a light theme is in scope.

2. **Push-based theme updates (dark, accent) with an accent index map.** What: settings change events push to the shell; accent is an enum mapped to a palette, early-return if unchanged. Why: Lingmo uses signals, not polling (`thememanager.cpp:82-84`, `:206-210`). Files: `crates/flamewm-shell/src/settings_projection/`, `crates/flamewm-settings/`, `crates/flamewm-platform/src/settings.rs`. Cost: one event subscription, no per-frame work. Verdict: **ADAPT**. Only through the existing typed settings facade. The shell must not read config files itself and must not own theme state.

3. **Dock/panel radius as a fraction of thickness, compositing-gated.** What: `radius = 0.3 x thickness` when compositing, square otherwise (`DockPanel/qml/main.qml:15,36`). Why: consistent pill feel at any dock size. Files: `crates/flamewm-shell/src/dock.rs`, `crates/flamewm-shell/src/panel.rs`, `ui/shell/flamewm.css` (`--radius-*`). Cost: none. Verdict: **ADAPT** as an opt-in token. Changing the default radius changes the look, which the task forbids.

4. **Shadow presets as a small table of layered shadows, computed once.** What: 4-5 size presets, each 2-3 layers `(offset, radius, alpha)` (`oceanshadowhelper.cpp:44-70`), tiles cached (`:210-217`). Why: predictable elevation. Files: `ui/shell/flamewm.css` (box-shadow), or `crates/flamewm-image-core/` if shadows are rasterised. Cost: a one-time render per preset; memory = tile size. Verdict: **REJECT** for porting numbers. FlameWM's surface look must not change, and Lingmo's numbers are unmeasured here. Keep the caching idea only if FlameWM later rasterises shadows.

5. **Wallpaper-blur backdrop for the launcher, cached.** What: a blurred copy of the wallpaper behind the launcher, with a black overlay at 0.4 / 0.5 (`Launcher/qml/main.qml:174-197`). Why: legibility. Files: `crates/flamewm-shell/src/surface_backdrop.rs` (108 lines, `desktop_source` at l.20), `crates/flamewm-image-core/src/blur.rs`. Cost: Lingmo's version recomputes every frame (`cached: false`), which is a regression. A FlameWM version must blur once per (wallpaper, size) and reuse the result. Verdict: **ADAPT** only with a cache keyed on wallpaper path, size and radius, and with the cache released on wallpaper change. Verify the existing `surface_backdrop.rs` first; it may already do this.

6. **Status-button icons loaded asynchronously.** What: icon items use `asynchronous: true` (`StatusBar/qml/IconButton.qml:74`). Why: no UI stall on first paint. Files: `crates/flamewm-shell/src/icon_loader.rs`, `crates/flamewm-shell/src/tray.rs`, `crates/flamewm-shell/src/async_projection.rs`. Cost: none, slightly more work on first frame. Verdict: **ADOPT** the concept if `icon_loader.rs` is still synchronous on the UI path. Confirm first; the async projection module may already cover it.

7. **Icon provider fallback chain with size clamp.** What: clamp size to >= 1, path or theme name, fallback to a generic app icon (`iconthemeimageprovider.cpp:31-50`). Why: no blank icons. Files: `crates/flamewm-shell/src/icon_loader.rs` (814 lines), `crates/flamewm-shell/src/icon_cache.rs` if present. Cost: none. Verdict: **REJECT** (already covered by FlameWM's loader, which is larger and more complete). Only add a regression test if the fallback is missing.

8. **Paged launcher with 300 ms InOutQuad page slide.** What: all-apps paging animated with `NumberAnimation` 300 ms InOutQuad (`Launcher/qml/AllAppsView.qml:60-64`). Why: spatial continuity. Files: `ui/shell/flamewm.css` (if CSS transition) or `crates/flamewm-shell/src/launcher_surface.rs`. Cost: one animation per page turn. Verdict: **ADAPT** the timing only if FlameWM's launcher pages. Do not copy the `cacheBuffer` pre-instantiation (`AllAppsView.qml:55`), which is a memory cost. Verify with `grep -n transition ui/shell/flamewm.css` before choosing a duration.

Rejected outright, for the record: launcher `cacheBuffer = width * count` (memory); `FastBlur cached: false` per frame (CPU/GPU); synchronous wallpaper decode with `cache: false` (UI stall); the commented-out launch animation (dead code).

## 9. Not verified

- No runtime, no screenshots, no timing measurements. All perf statements are source-read judgements.
- FlameWM CSS transitions and launcher motion were not enumerated; the verdicts for ideas 3, 6, 8 depend on that check.
- `shell32/` (KDE-derived Plasma stack) was mapped, not analysed.
- `fm/` (file manager), `core32/`, `im/`, `installer/` were not analysed.
- Lingmo status tray, notifications and desktop widgets were only listed.

## 10. Next steps (one action each)

1. `grep -n 'transition\|animation' ui/shell/flamewm.css` to fix the motion baseline for ideas 6 and 8.
2. `grep -n 'async\|cache' crates/flamewm-shell/src/icon_loader.rs crates/flamewm-shell/src/async_projection.rs` to confirm idea 6 is not already done.
3. Read `crates/flamewm-shell/src/surface_backdrop.rs` in full before deciding idea 5.
