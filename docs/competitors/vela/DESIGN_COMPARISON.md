# Vela vs FlameWM: design consistency comparison

Snapshot: 2026-11-08. Competitor screenshots are from `.vendor/vela-main/docs/screenshots/`. FlameWM screenshots are from `assets/screenshots/`. No new Xvfb session was started: host load was about 28 on 2026-11-08, and the existing images were enough.

## Evidence used

| Source | Images viewed | Not viewed |
| --- | --- | --- |
| Vela | `start.webp`, `files.webp`, `panels.webp`, `light.webp`, `lock.webp` | `alt-tab`, `context-menu`, `snap`, `snap-layouts`, `task-view` |
| FlameWM | `overview.webp` (Terminal, Explorer, widgets, taskbar), `start-menu.webp` (launcher, dock) | `desktop.webp` (close to `overview.webp`), no settings, lock, notifications or terminal-only capture |

Limits:

- Vela images show Italian UI and a 1920x1080 frame. FlameWM images are 1600x900 or 1344x756 and contain sample files and a neutral profile.
- The palette and radii below are visual estimates from the screenshots. They were not sampled with a colour tool. Treat them as approximate.
- Scores are design-consistency judgements, not usability tests.

## 1. Scorecard (1 to 10)

`n/c` means the surface was not captured in the viewed images. `n/a` means the surface does not exist in that desktop.

| Surface | Vela | FlameWM | Justification (screenshot) |
| --- | --- | --- | --- |
| Taskbar | 8 | 6 | Vela: centered icons, one blue active marker, right-side tray and clock with date, all on one dark tint (`files.webp`, `start.webp`). FlameWM: flat bottom bar with logo, workspace grid (1 to 4), weather, tray glyphs and clock (`overview.webp`). The FlameWM taskbar is consistent but looks more utilitarian; glyph sizes differ from the surrounding text. |
| Dock | n/a | 6 | Vela has no dock. FlameWM's pill dock (`start-menu.webp`) is a rounded dark capsule with two icons and dot indicators, consistent with the glass language. `docs/DOCK_DESIGN.md` still says "partial", so the dock is an early surface. |
| Settings | 8 (quick settings) / n/c (app) | n/c | Vela quick settings (`panels.webp`): rounded tiles with a blue active state, labelled icons, a volume slider and an expand chevron on tiles. The Settings app is not in the viewed images. FlameWM Settings is not captured. |
| Explorer | 8 | 7 | Vela (`files.webp`): pinned sidebar with accent icons, breadcrumb bar, search field, monochrome toolbar, thumbnails in grid, consistent 8 to 10 px radius, consistent light and dark pair (`light.webp`). FlameWM (`overview.webp`): Places sidebar, Date modified and Size columns, path bar, free-space meter in the status bar. Consistent dark glass. It is less dense in icon detail and has no thumbnails in the captured view. |
| Terminal | n/c | 6 | FlameWM terminal (`overview.webp`, `start-menu.webp`): translucent dark body, neofetch-style output, title bar with chevron-style window buttons. The terminal shares the Explorer title bar, which keeps the two windows consistent. Vela terminal is not captured. |
| Lock screen | 8 | n/c | Vela (`lock.webp`): full-bleed wallpaper with a large light clock and a date line. Minimal, high contrast, no clutter. FlameWM lock screen is not captured. |
| Notifications and calendar | 8 | n/c | Vela (`panels.webp`): grouped cards by app with app icons, timestamps, a "Clear all" action and a calendar with an accent-coloured current day. Consistent radius and spacing. FlameWM notification surface is not captured. |
| Launcher | 8 | 7 | Vela (`start.webp`): search field with blue focus ring at top, sections "Pinned" and "All apps", icon grid with labels, user row and power button at the bottom. Acrylic blur behind the panel. FlameWM (`start-menu.webp`): search field at bottom, two-column list of categories and apps, no icon grid, translucent blur behind. Coherent, but sparser and text-heavy. |
| Widgets | n/a in Vela (no widgets in README) / 7 | n/a / 7 | FlameWM widgets (`overview.webp`): rounded dark tiles for clock, CPU ring (blue), memory ring (orange), media tile. Good spacing. The two ring colours do not match the taskbar accent, which weakens palette consistency. Vela has no widget surface. |

Averages over scored surfaces: Vela 8.0 (6 surfaces scored: taskbar, quick settings, Explorer, lock, notifications, launcher), FlameWM 6.5 (6 surfaces scored: taskbar, dock, Explorer, terminal, launcher, widgets). FlameWM is scored lower overall for consistency, not for raw polish of any single surface. The averages are informational only.

## 2. Palette

| Aspect | Vela | FlameWM |
| --- | --- | --- |
| Base | Dark navy and teal wallpaper with a dark grey shell tint (`start.webp`, `lock.webp`). Light mode is cool white (`light.webp`). | Dark grey glass over a mountain or pastel wallpaper (`overview.webp`, `start-menu.webp`). |
| Accent | One blue accent used for active states, toggles, focus rings and the current day (`panels.webp`, `start.webp`). | No single accent across surfaces. Taskbar workspace grid uses blue for the active cell; CPU ring is blue; memory ring is orange; terminal and Explorer use neutral greys. |
| Text | Off-white primary, mid-grey secondary. Consistent. | Off-white primary, mid-grey secondary. Consistent. |
| Wallpaper integration | Wallpaper-tinted translucent title bar (README L197-199). | Glass tint picks up the wallpaper (`start-menu.webp`). |

Gap: FlameWM has no visible accent system across surfaces. Vela applies one accent to every active state.

## 3. Glass, blur and transparency

- Vela: live acrylic blur behind the shell, applied through `ext-background-effect-v1` (README L262-263). Visible in `start.webp` behind the launcher and in `panels.webp` behind quick settings. Blur is strong enough that wallpaper detail is lost.
- FlameWM: translucent glass with blur on launcher and terminal windows (`start-menu.webp`: the terminal shows through the Start menu and the menu is tinted over the wallpaper). The blur is softer and the tint is darker than Vela's.
- Verdict: both use glass. Vela's is more uniform across the shell. FlameWM's is more visible in its launcher and dock.

## 4. Radius, spacing and icons

| Aspect | Vela | FlameWM |
| --- | --- | --- |
| Window corner radius | Rounded at about 10 px at 1920 px width (`files.webp`). Compositor-drawn shadow (README L195-196). | Rounded at about 8 to 10 px at 1600 px width (`overview.webp`). Shadow present but softer. |
| Tile and card radius | About 12 px on quick settings tiles and notification cards (`panels.webp`). | About 12 px on widget tiles and dock capsule (`overview.webp`, `start-menu.webp`). |
| Spacing rhythm | Consistent 8 px grid visible in tile gaps and list padding. | Consistent gaps in widgets and launcher. Explorer list row height looks tighter than Vela's. |
| Icon style | Full-colour app icons from the user's icon theme in launcher and taskbar, monochrome glyphs in toolbars (`start.webp`, `files.webp`). Mixed app icon sets (Firefox, Dolphin, Kate, Gwenview) make the launcher busy. | Flat monochrome glyphs in taskbar and terminal; folder and file icons are flat-blue (`overview.webp`). Few app icons appear in the launcher. |

## 5. Typography

- Both use a humanist sans (Noto Sans or Open Sans family in the captures). Hierarchy in both is clear: title, body, caption.
- Vela uses a larger clock on the lock screen and a clear date line. FlameWM's widget clock is compact.
- FlameWM terminal output uses a monospace font with good line spacing (`overview.webp`).

## 6. Consistency findings for FlameWM

1. No shared accent colour across taskbar, widgets, launcher and dock. Vela uses one.
2. Taskbar glyph sizes are not aligned to the text baseline. Vela's taskbar icons and clock align cleanly.
3. The launcher is text-heavy and has no icon grid. Vela's launcher has a labelled icon grid.
4. Dock is partial. The dock design doc is incomplete, and the visual form is not yet tied to the taskbar.
5. Settings, lock, notifications and terminal-only surfaces were not captured in `assets/screenshots/`. Their consistency is unverified.

## 7. Consistency findings for Vela

1. Full-colour app icons in launcher and taskbar increase visual noise when many apps are installed, as in `start.webp`.
2. Light mode is captured only for Explorer (`light.webp`). Light-mode consistency for other surfaces is not in the evidence.
3. Some strings are localized (Italian in the captures), which is good, but the screenshots are one locale only.

## 8. Recommended design actions for FlameWM (input to the gap list)

1. Define one accent token and apply it to active states on taskbar, widgets, launcher, dock.
2. Align taskbar glyph size and baseline; check against Explorer and terminal title bars.
3. Add an icon grid option to the launcher, or accept the text list as a deliberate style and document it.
4. Capture the settings, lock, notifications and terminal surfaces into `assets/screenshots/` so consistency can be scored with evidence.
5. Finish the dock per `docs/DOCK_DESIGN.md` so it can be scored.

## 9. Files

- Vela images: `.vendor/vela-main/docs/screenshots/{start,files,panels,light,lock}.webp` (viewed), plus `alt-tab`, `context-menu`, `snap`, `snap-layouts`, `task-view` (not viewed).
- FlameWM images: `assets/screenshots/overview.webp`, `assets/screenshots/start-menu.webp` (viewed), `assets/screenshots/desktop.webp` (not viewed separately).
- Feature inventory: `docs/competitors/vela/FEATURES.md`.
