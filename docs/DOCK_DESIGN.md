# Dock design

Status: partial. The pure model (`crates/flamewm-shell-core/src/dock.rs`) exists. The shell surface,
settings wiring, work-area hook and visual checks are not built yet (see "Not yet done").

## 1. How the macOS Dock works

- **Two groups, one row.** The Dock shows pinned apps (persisted by the user) followed by running apps
  that are not pinned. A separator divides the groups and is drawn only when both are non-empty.
- **Running indicator.** A small dot sits under (or beside, on a vertical dock) each app that has at
  least one open window. Pinned-but-closed apps show no dot.
- **Click.** Launches a closed app. Focuses the app's most recent window if it is running. Clicking
  the already-focused app minimises or cycles its windows.
- **Context menu.** Keep in Dock / Remove from Dock, Open, Quit (close its windows), Show in Finder.
- **Reorder.** Drag a pinned icon along the bar. Other icons slide to make room.
- **Add/remove.** Drag an app onto the Dock to pin it. Drag an icon off the Dock to unpin it.
  Unpinned running apps vanish when their last window closes.
- **Hover.** A static label with the app name appears above the icon.
- **Placement.** Bottom by default. Left, right or top are also possible. The Dock reserves screen
  space (maximised windows stop at its edge) and can be hidden.

## 2. What FlameWM replicates

| Behaviour | Status |
| --- | --- |
| Pinned + running merge, pinned first, separator, unpinned running last | Done: `merge_dock` (unit tested) |
| Pinned list add/remove/reorder/dedupe with cap 32 | Done: `pin_app`, `unpin_app`, `move_pinned` |
| Running dot per app | Planned: reuse taskbar activity indicator tokens and projection |
| Click launch/focus/cycle | Planned: reuse taskbar click logic |
| Context menu | Planned: reuse `flamewm-ui-core` context menu model |
| Drag reorder / drag to pin or unpin | Planned: reuse shell pointer drag mechanism |
| Hover name label | Planned: reuse existing tooltip mechanism if present |
| Vertical variant | Planned: one template with orientation |
| Work-area reserve (maximise respects the Dock) | Planned: reuse taskbar strut/work-area path |
| Taskbar gradient (`taskbar.style = gradient`) | Pure geometry done: `gradient_background`; CSS wiring planned |

## 3. What FlameWM skips

- No bounce, magnification or any transition. The user asked for no animation.
- No Trash or Downloads folder items.
- No Stacks or Hot Corners integration.

## 4. Constraints

- No always-on timers below 50 ms, and no new always-on threads. The dock uses shell event sources.
- Colours come from `:root` tokens in `ui/shell/flamewm.css`, mirrored in
  `flamewm-skin` `shell_tones.rs` `FIXED_TONES`. No raw colours in templates.
- One surface, created only when `dock.enabled` is true.
- `flamewm-wm-*` changes are limited to the work-area hook.

## 5. Not yet done

- Shell surface `ui/shell/dock.html` and vertical variant.
- `flamewm-platform` settings keys (`dock.*`, `taskbar.enabled`, `taskbar.style`,
  `taskbar.gradientEdge`). The contract is in `docs/DOCK_SETTINGS_SPEC.md`.
- Work-area reserve in `flamewm-wm-x11`/`flamewm-wm-wayland`.
- Click, drag and context-menu wiring; Xvfb screenshots; xtest pointer checks.
