# u68-dock-hover log

Batch ten over `crates/flamewm-shell/src/dock.rs` (the only owned path; the shell layout/animation
owner in `app/dock.rs` belongs to u69). `git` and `cargo` are forbidden in this shared checkout, so
this is a source-level pass and the orchestrator builds.

## Review of the landed slices

- `LOG-u07-shell-icons-dock.md`: the cached `dock_node_ids` arrays and the linear `running_apps`
  dedupe are intact; no regression found.
- `LOG-u46-shell3.md`: the in-place hover hit test, the `dock_enabled` / `dock_auto_hide` peeks and
  the `dock_hover_scale` / `dock_launch_bounce` peeks all still gate the hover and click paths. The
  remaining per-rebuild work was in this file, listed below.

## Composition of a 1 s hover+click sample

- Hovering (pointer motion, no slot change) already costs zero allocation and no rebuild.
- A hover change writes two icon rects; a click activates/launches and usually triggers one dock
  rebuild. In that rebuild the largest share was document mutation traffic from `project_dock`
  (up to 72 `visible` writes just to hide the 24 fixed slot nodes), then the `layout_for` app-id
  scan/clones, then the per-slot geometry constants recomputed in `dock_frame`.

## Changes

1. **Unused slots hide one node, not three** (`project_dock`). The icon and dot are children of the
   slot button, and paint/layout treat a hidden ancestor as hidden (`is_effectively_visible` walks
   parents), so hiding a slot node already hides its children. Up to 48 redundant document writes
   per rebuild (2 per empty slot) are gone; a typical five-icon dock drops from ~102 to ~54
   `visible` calls, and the projection now loops only the shown slots before the hide pass.

2. **`layout_for` skips the running-app scan when it cannot be used.** `merge_dock` ignores the
   running list when `show_running` is off, so the old code still walked every window and cloned
   every unique app id for nothing on each rebuild. It now passes an empty list in that mode.

3. **`running_apps` stops at `MAX_SLOTS`.** Only the first 24 slots are ever projected, so the scan
   now breaks once 24 unique apps are collected instead of cloning ids for windows that can never
   appear. Bounds the per-rebuild allocation on a crowded desktop.

4. **`click_request` scans the window list once.** The old form ran the filter twice (focused
   search, then first candidate). It now returns on the first focused match or keeps the first
   candidate in the same pass; a dock click on an unfocused app no longer re-walks every window.

5. **`dock_frame` hoists per-slot constants.** The label row, label along-axis inset and dot inset
   depend only on the bar and slot size; they are computed once per frame instead of per slot.

No dead items were found in the owned file; `rg` shows every helper and constant still has a caller.

## Expected composition shift

Rebuilds (click, window/app change) spend a smaller share on document mutation and app-id
allocation; idle hover remains unchanged and allocation-free. No visual or behavioural change:
slot/icon/dot rects are numerically identical, hidden children stay effectively hidden, and click
selection order (focused first, then first in window order) is preserved.
