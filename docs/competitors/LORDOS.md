# LordOS dock analysis (reference only)

Scope: `.vendor/LordOS-main/` (read-only). Ideas only, no code copied. Research only: no cargo, no builds, no repo source edits.

## 0. Licence and status

- LordOS: **MIT**, `.vendor/LordOS-main/LICENSE` (`MIT License, Copyright (c) 2026 iPERFEX and the LordOS contributors`); `THIRD_PARTY.md` confirms MIT for its own code. Permissive, but this report follows the brief: ideas only.
- FlameWM: **LGPL-2.0-only** (`docs/README` license section). Not a licence blocker for ideas.
- Note: one indexed licence snippet said GPL-3.0-or-later; that came from the LingmoOS/Vela tree, not LordOS.
- The LordOS dock is a JavaScript/React-style theme on QuickJS (`THIRD_PARTY.md`), with its own `wm`/`fs`/`settings` SDK. FlameWM is native Rust/X11/Wayland with typed facades. Architecture fit is low, so only behaviour and layout ideas transfer.

## 1. The dock

### 1.1 Source files

| Concern | LordOS file | Evidence |
|---|---|---|
| Model: pinned ids, typed ids, rules (Files/Settings/launcher locked, Trash last) | `sdk/dockmodel.js` | header rules, `LOCKED = [FIRST, LAUNCHER, 'app:settings']`, `DOCK_KINDS` |
| SDK logic: entries, activate, menu, pin/move/unpin/reset, drag, poof, trash | `sdk/dock.js` | API header lines 1-10; `dock.pin/move/unpin/reset` ~L109-160; trash stat logic |
| Visual: magnification, auto-hide, tooltip, drag gap, poof, trash, dots | `themes/default/components/Dock.jsx` (463 lines) | constants L16-20; render L415-460 |
| Settings UI | `apps/settings/Dock.jsx` | `MenuBarSection`, `ControlCenterSection`, desktops section |
| Tests | `server/tests/dock_model_test.mjs` | model-only tests |
| Spec doc | `docs/DOCK.md` (270 lines) | settings table L28-37, reorder L76-80, remove L82-86, trash L173-193, auto-hide L210-236 |

### 1.2 Layout and metrics (Dock.jsx, theme default)

| Value | LordOS | Source |
|---|---|---|
| Icon size (default / range) | 54 px / 36-80 px | `Dock.jsx` L216 default `size=54`; `docs/DOCK.md` L33 |
| Magnified max | 86 px (range up to 128) | `maxSize = 86`, `docs/DOCK.md` L35 |
| Icon gap | 8 px (`GAP`) | `Dock.jsx` L16 |
| Edge padding | 9 px (`PAD`) | L16 |
| Outer margin from screen edge | 6 px (`MARGIN`) | L16 |
| Separator | `SEP = GAP + 1` (9 px slot); 1 px line, 70% of base height | L17, L422-423 |
| Bar radius | fixed 20 px | L419 |
| Bar fill | theme `t.dockBg` + `backdropFilter: blur(t.blur)`; border `1px` `t.dockBorder` | L419-420 |
| Icon radius | 22.5% of icon size | L430 (trash), L30 (placeholder) |
| Dot (running) | 4 px circle, 7 px outside the bar edge, colour from theme | L369-370, L440 |
| Tooltip | pill, padding 10x4, radius 7, 12 px / 500 weight | L378-380 |
| Remove badge | 18 px circle, `×` 12 px / 700 | L407-409 |
| Remove cue label | 11 px / 600, padding 8x3, radius 6 | L400-402 |
| Dark-mode body | `linear-gradient(rgba(235,238,245,.30), rgba(200,205,215,.16))` | L43 |
| Light-mode body | `linear-gradient(rgba(255,255,255,.85), rgba(215,220,230,.65))` | L43 |
| Dark/light outline | `rgba(255,255,255,.55)` / `rgba(60,66,80,.45)` | L44 |
| Dot colour | dark `rgba(255,255,255,.85)`, light `rgba(0,0,0,.7)` | L370 |
| Placeholder icon | `linear-gradient(#5b5b66, #2b2b33)`, shadow `0 0.04s 0.12s rgba(0,0,0,.35)` | L31-32 |

Shadow on placeholder and dock: `0 0.03s 0.10s rgba(0,0,0,.25)` on the Trash/bar layer, L60.

### 1.3 Motion (durations and curves)

| Motion | Duration | Curve | Source |
|---|---|---|---|
| Slide in/out (auto-hide) | 180 ms | ease-out cubic (`1-(1-k)^2` in rAF) | `SLIDE_MS` L18; L99 |
| Reorder glide (neighbours) | 150 ms | ease-out cubic (`1-(1-p)^3`) | `REORDER_MS` L19; L164 |
| Poof (removal) | 320 ms | linear 1-p fade + scale | `POOF_MS` L20; L187-210 |
| Cover delay (overlap mode) | 300 ms | timer | `COVER_MS` L79 |
| Reveal on edge touch | 120 ms wait, then 180 ms slide | timer | `docs/DOCK.md` L215 |
| Hide after pointer leaves | 500 ms (8 px margin) | timer | `docs/DOCK.md` L216 |
| Magnify | none (instant) | cosine falloff over `RANGE = 3 * BASE` | L263, L279-281 |
| Hover tooltip | none | instant | L374-380 |

Magnify: `size = BASE + (MAX-BASE)*cos(d*pi/2)` for `d = |pointer - icon centre| / RANGE`, `d<1` only. It is recomputed on every `mousemove` (`setM` L417). Dock width grows by about 2.6 icon widths; the fit loop shrinks BASE so it still fits (L258-262).

### 1.4 Running indicators, separators, trash

- **Running dot**: a 4 px dot below (or beside, on side docks) each running app. Shown per item (`running` flag from `useDockEntries`, dock.js). Not a badge.
- **Separator**: inserted before recent apps and before the Trash (`sepDone`, L236, L389-390); `SEP` width, 1 px line.
- **Trash**: always last, after a separator, cannot be moved or removed (`docs/DOCK.md` L90). Full/empty icon from `~/.local/share/Trash/files`:
  - `stat` every 2 s only while a Dock shows it (mtime change), full list only on change or every 30 s (`dock.js` trash block, `docs/DOCK.md` L183-184). Never a busy loop.
  - Click opens Files on Trash; drop Files nodes on it to trash; drop a Dock icon on it to unpin; right-click menu Open / Empty Trash (confirm first).

### 1.5 Drag, pin/unpin, context menu, badges, previews

- **Drag reorder**: press starts drag after 6 px (`DRAG_START`, dock.js). Neighbours slide 150 ms; drop index passed to `dock.move`. Works with touchpad, RTL, left/right docks; Escape cancels (`docs/DOCK.md` L76-80).
- **Remove**: drag more than about 60 px away and hold 350 ms (`REMOVE_HOLD`, dock.js) to show a «Remove» cue, release to unpin with a poof. Dropping on Trash or menu «Remove from Dock» does the same.
- **Pin**: drag from launcher or running app; right-click «Keep in Dock» (`docs/DOCK.md` L66-74).
- **Context menu**: quick actions (per-app, from manifest), windows, app items (`registerDockMenu`), New Window, Options (Keep/Remove, Open at Login, Assign To display), Show All Windows, Hide, Quit, Open (`docs/DOCK.md` L143-147). Keyboard navigable.
- **Badges**: none found (no notification counts in the dock code). **Previews**: none in the dock; window list is only in the menu.

### 1.6 Auto-hide, multi-monitor, edges, settings

- Three modes: `never`, `overlap` (hide only while a window covers the dock's rest place; `dockAutohide` + `dockIntellihide`), `always` (`sdk/dockmodel.js` L32-37 comment; `dockHideMode()`).
- Cover detection uses the window list, including Wayland and X11 windows, and fullscreen covering the display (`docs/DOCK.md` L230-236). Dock is on the main display only; "Use as Main Display" moves it (`docs/SETTINGS.md` L103).
- Edges: bottom, left, right. RTL mirrors the bottom dock only (`Dock.jsx` L1-4).
- Layer order: dock layer, popover above it, tooltip 80, drag 90, system 100 (`docs/LAYERS.md`).
- Settings exposed (`docs/DOCK.md` L28-42): `dockSize` 36-80 (54), `dockMagnify` (true), `dockMagnifySize` 54-128 (86), `dockAutohide` (false), `dockIntellihide` (false), `dockPinned` (list), `appDisplays` (map), dock position, desktops switch, reset.

## 2. Source map and architecture

| Layer | LordOS | Notes |
|---|---|---|
| Runtime | QuickJS JS engine (`third_party/quickjs`), React-like component API | `THIRD_PARTY.md` L1-10 |
| SDK | `sdk/*.js` (~130 modules): `dock`, `dockmodel`, `layers`, `motion*`, `apps`, `settings`, `store`, `windows` | `sdk/index.js` |
| Theme | `themes/default/components/Dock.jsx`; Lord Classic taskbar is a second theme | `docs/DOCK.md` L238-244 |
| Native | `hw/`, `kms.c`, `lord-server` C engine; damage/paint in C | `docs/LAYERS.md` mentions `lord-server`, `paint.c` |
| IPC | SDK calls to the C server (settings file `~/.config/lordos/settings.json`); no typed facade layer | `docs/DOCK.md` L15 |
| Icons | `Icon` component passed per entry; `windowIcon(w, getApp)` fallback; `LetterIcon` fallback | `sdk/dock.js` (4) |
| Icon cache | not found in dock code; icons come from app manifest / `.desktop` | not verified in depth |

Module-level contract is clean: the model (`dockmodel.js`) is pure and tested; the SDK owns mutations; the theme only draws (`Dock.jsx` L8-9: "this file only draws it").

## 3. Responsiveness techniques

- **Pure model, separate draw**: dock list is recomputed from store subscriptions; no per-frame model work.
- **Redraw only the Dock**: auto-hide animation "redraws only the Dock, not the desktop" (`docs/DOCK.md` L220). Blurred surfaces widen repaint to twice the blur radius (`docs/LAYERS.md`, "Blurred surfaces and partial repaints").
- **Idle cost**: trash stat 2 s; reveal and hide use timers; rAF only while animating (`useGlide` L158-168 stops rAF when `running.current` is false).
- **Async**: icon loading is delegated to the SDK; none inspected in detail.
- **Frame pacing**: rAF-driven glide with cubic ease. Reduce-motion collapses durations to 0 (`docs/DOCK.md` L77).
- **Blur**: `backdropFilter: blur()` on DOM elements, rendered by the LordOS C compositor/paint path. Blur cost is high; LordOS docs call it out.

## 4. Design tokens (LordOS dock)

- Colours: see 1.2 (dock body, outline, dot, placeholder gradient). Theme tokens `t.dockBg`, `t.dockBorder`, `t.panelBg`, `t.windowBorder`, `t.blur`, `t.text`, `t.mode` (dark/light) are defined by the theme, not fetched in this pass.
- Fonts: 12 px / 500 (tooltip), 11 px / 600 (remove cue), 12 px / 700 (remove ×).
- Spacing: 6, 8, 9 px (gap, padding, margin); radius 6, 7, 20 px; 22.5% icon radius.
- Motion: 120, 150, 180, 300, 320, 350, 500 ms.

FlameWM equivalents (`ui/shell/flamewm.css`):
- `--panel: #000000bf`, `--surface: #000000bf`, `--rim: #ffffff7f` (L3). Dock bar uses `background: var(--panel)`, `border: 1px solid var(--rim)`, `border-radius: var(--radius-xl)` (L328).
- Slot hover `var(--hover-fill)` (L330); dot `var(--accent)`, 4 px (`dock.rs` L143, css L332); separator `var(--rim)` (L333); menu `var(--surface)` (L335); font `--font-sm` (L338).
- No `backdrop-filter` or `box-shadow` in `ui/shell/flamewm.css` (grep, L1-341).
- Radii are token-based (`--radius-*`); exact px values not read in this pass.

## 5. Visual comparison: LordOS dock vs FlameWM dock

Evidence: FlameWM screenshot `/tmp/claude-1000/-run-media-linsaftw-SDD-GitHub-ArkFlame-flamewm-webrender/84cf5e3a-57bb-4dfd-9c4b-adc66fab352b/scratchpad/vis/out/edge-bottom/1-revealed.png` (two gear icons in a pill, 1280x800 scaled). Reference screenshot `/home/linsaftw/Downloads/desktop-overview-dark.png` (macOS-like dock: about 10 icons, pill, dots, separator, trash).

| Aspect | LordOS (reference) | FlameWM (current) | Gap |
|---|---|---|---|
| Shape | pill, radius 20 px fixed | pill, radius = half bar thickness (`dock.rs` L425, `pill_corner_radius`) | equal |
| Fill | gradient translucent + blur | `--panel` 75% black, flat, no blur | FlameWM lacks gradient and blur |
| Border | 1 px light, 0.45-0.55 alpha | 1 px `--rim` (50% white) | close |
| Icon size | 54 px default, 36-80 | 44 px default, 24-96 (`dock.rs` L31, L56) | FlameWM smaller by default |
| Padding / gap | 9 px / 8 px | 6 px / 3 px (`dock.rs` L132-133) | FlameWM denser |
| Magnify | yes, cosine, 86 px | no | big visual gap |
| Running dot | 4 px, below | 4 px, below (`dock.rs` L143, L237-248) | equal |
| Separator | yes (before Trash and recents) | yes (`separator_after`, `dock.rs` L276-292) | equal |
| Trash | yes, always last, full/empty icon | no trash in shell source (grep: 0) | missing |
| Drag reorder | yes, glide 150 ms | not in `crates/flamewm-shell/src/dock.rs` (grep: 0 drag) | missing |
| Remove by drag | yes, 60 px + 350 ms hold | menu only (`DockMenuAction::RemoveFromDock`, shell-core `dock.rs` L142-147) | missing drag |
| Context menu | Keep/Remove/Open/Options/Hide/Quit + quick actions | Keep, Remove, Open, Close windows (`menu_items`, shell-core L142-160) | FlameWM narrower |
| Tooltip | pill, 12 px / 500, radius 7 | label 160x22, `LABEL_W`/`LABEL_H` (`dock.rs` L140-141) | similar |
| Auto-hide | never / when covered / always | on/off `auto_hide` (`dock.rs` L21-22, L59); reveal strip 3 px (L137) | missing "when covered" |
| Multi-edge | bottom, left, right | `Edge` enum incl. top (`dock.rs` L17) | FlameWM broader |
| Badges / previews | none | none | equal |
| Motion | 150-320 ms curves | none on dock | missing |

Read: FlameWM already matches LordOS on shape, dots, separators, auto-hide base and edges. Visible gaps are magnify, trash, reorder, the remove cue, the "covered" auto-hide mode, and the glassy fill.

## 6. Ranked improvements for FlameWM (max 8)

Cost baseline: `docs/performance` table in `docs/PERFORMANCE*` (pointer sweep over taskbar: 2.62 to 1.75 % CPU, Xorg 1.62 to 1.50 %). Every repaint is still full-window (`FLAMEWM_RENDER_REGION` opt-in). GPU blur: CPU 34.8 ms vs GPU 10.8 ms per r12 blur, but GPU adds about 143 MB PSS (`docs/GPU*`), so default stays `cpu`. Any change must keep idle CPU and memory flat.

1. **ADAPT: Trash slot at end, after a separator, full/empty icon, click opens Files Trash.**
   - What: a non-movable slot after the separator; icon state from a stat of `~/.local/share/Trash/files`, polled every 2 s only while the dock is visible, full list only on change or every 30 s.
   - Why: LordOS's clearest affordance; matches the reference screenshot.
   - Files: `crates/flamewm-shell-core/src/dock.rs` (layout item kind), `crates/flamewm-shell/src/dock.rs` (projection), `ui/shell/dock.html` / `flamewm.css` (icon slot), trash state through the existing typed platform facade (not in UI).
   - Cost: one `stat` every 2 s while shown (microseconds), and rare list reads. Hold the poll on a control-signal or inotify source if one exists; a timer is acceptable.
   - Verdict: **ADAPT**. Do the stat in the shell's service layer, not in the UI document. No UI-side authority over file operations.

2. **ADAPT: Drag-to-reorder with 6 px threshold, drop-index only, cancel on Escape.**
   - What: pointer press on a slot starts a drag after 6 px; drop calls a typed `dock.move` request. Neighbours glide 150 ms using CSS transitions, not a JS rAF loop.
   - Why: LordOS's reorder is pure model + gap in the draw. FlameWM already has pure layout (`dock_frame`, `layout_for`).
   - Files: `crates/flamewm-shell-core/src/dock.rs` (`merge_dock` gets a move op), `crates/flamewm-shell/src/dock.rs` (drag state, hit test), `crates/flamewm-api` / control request for the write, `ui/shell/flamewm.css` (transition on slot `left/top`).
   - Cost: zero when idle; during drag, a few repaints of the dock rect only. The settings write happens once, on drop.
   - Verdict: **ADAPT**. Route the write through the settings facade; the UI never owns pinned state.

3. **ADAPT: "Hide when covered" auto-hide mode (`dock.hideMode = never | covered | always`).**
   - What: dock stays while no window covers its rest place; slides away 300 ms after a cover, returns on pointer at the edge after 120 ms; hides 500 ms after pointer leaves.
   - Why: LordOS's most-used mode; FlameWM already has `auto_hide` and a 3 px reveal strip (`dock.rs` L21-22, L137, `reveal_rect` L367).
   - Files: `crates/flamewm-shell/src/dock.rs` (`dock_settings`, L59-60), `crates/flamewm-shell-core/src/dock.rs`, WM snapshot listener in `crates/flamewm-desktop/src/main.rs` (window covered event from `WindowSnapshot`).
   - Cost: event-driven on window-stack changes only, no polling. Slightly more work per stacking change.
   - Verdict: **ADAPT**. Keep the existing `dock.autoHide` boolean readable for back-compat, as LordOS does (`sdk/dockmodel.js` L32-37).

4. **ADAPT: Drag-out-and-hold remove cue, instead of menu-only remove.**
   - What: a pinned slot dragged more than about 60 px and held 350 ms shows a «Remove» badge; release unpins. Drop on the Trash slot also unpins.
   - Why: LordOS's remove is discoverable and fast; FlameWM has only the menu action.
   - Files: same drag code as #2; `crates/flamewm-shell-core/src/dock.rs` (`DockMenuAction` reuse).
   - Cost: same as #2; one timer during drag.
   - Verdict: **ADAPT**, after #2 lands. Depends on #2.

5. **ADAPT (reduced): hover-only icon scale instead of magnification.**
   - What: scale only the hovered slot 1.0 to about 1.12 with a 150 ms transition; no neighbour re-layout.
   - Why: gives the macOS feel without the cost of LordOS's per-move re-layout.
   - Files: `ui/shell/flamewm.css` (`.dock-slot:hover .dock-icon`), `crates/flamewm-shell/src/dock.rs` if the icon needs a size token.
   - Cost: CSS transform on one element. On the CPU path it still repaints the dock rect; expect small extra CPU during hover sweeps. Must be checked with the existing `tools/flamewm_cpu_workloads.py` `hover-taskbar` workload before adopting. Not measured here.
   - Verdict: **ADAPT**, only with a before/after CPU run. Full cosine magnify is **REJECT** (below).

6. **ADAPT: Glassy fill (gradient + translucent border) on the dock body only.**
   - What: replace flat `--panel` with a two-stop gradient token (`--dock-fill-top`, `--dock-fill-bottom`) at the LordOS alphas (0.30 to 0.16 dark), keep the 1 px rim.
   - Why: visual parity with the reference screenshot; keeps the look without blur.
   - Files: `ui/shell/flamewm.css` (L328 `.dock-bar`), token block L3.
   - Cost: a gradient fill is cheap on the CPU path (one fill per dock repaint). No `backdrop-filter`.
   - Verdict: **ADAPT**. Do not add `backdrop-filter` to the dock; LordOS's blur is the expensive part, and FlameWM's GPU blur costs about 143 MB PSS.

7. **REJECT: cosine magnification with live re-layout on mousemove.**
   - Why: LordOS recomputes every icon size and the bar length on each pointer move (`Dock.jsx` L278-285, L417). FlameWM repaints full windows; this would turn each pointer sweep into a multi-slot relayout and repaint. Conflicts with the perf contract. Also makes the dock grow over the screen edge, which the strut/work-area code (`crates/flamewm-wm-x11/src/wm/outputs.rs` L92-95) does not model.
   - Verdict: **REJECT**. Use #5 instead.

8. **REJECT: bounce on launch and «poof» removal animation.**
   - Why: a rAF loop per event (`Dock.jsx` L187-190, POOF 320 ms), an extra layer for the poof, and no state FlameWM needs. Cosmetic; costs frames on every launch. Also would need a drag layer the compositor does not have.
   - Verdict: **REJECT**.

Also rejected, not in the top 8: window previews on hover (needs per-window thumbnails; the compositor owns that, not UI), per-element `backdropFilter` blur (LordOS's approach, costs memory), a QuickJS/React runtime for the dock (contradicts the native typed-facade architecture), and badges (LordOS has none).

## 7. Architecture check (AGENTS.md rules)

- One owner per mechanism: pinned state and trash I/O must live in the shell service and facade layer, not the UI document (`ui/shell/flamewm.css` and `dock.html` only draw).
- UI never becomes WM authority: LordOS's `wm.open` and drop-to-remove paths map to FlameWM control requests, not direct UI calls.
- `.vendor/**` stays reference only; no imports.
- No new exceptions to `docs/ARCHITECTURE_EXCEPTIONS.toml` are needed for the ADAPT items.

## 8. Not checked

- `sdk/dock.js` lines 160-400 (full drag/poof/menu code) only partly read; theme token values (`t.dockBg` etc.) not fetched.
- LordOS icon loading and cache: not traced.
- FlameWM `ui/shell/dock.html` template and exact `--radius-*` px values: not read.
- One `bounce` hit in `crates/flamewm-shell`/`ui/shell` was not inspected (count 1 from grep).
- No runtime or CPU measurement was run (research-only task; the CPU and GPU numbers quoted come from existing repo docs, whose exact file paths were not re-verified: cite as "repo perf/GPU docs").
- File paths named as FlameWM integration points (window-covered event source in `crates/flamewm-desktop/src/main.rs`, `ui/shell/dock.html`, control-request write path) are proposals, not verified call sites.
