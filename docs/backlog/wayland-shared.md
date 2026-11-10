# Wayland/X11 shared-logic audit

Base: `origin/main` at worktree creation. Method: function-level reads of both sides (not name or size
judgement). Verdicts: **identical** (same output for every input), **near** (same output for the
reachable inputs, differing edge case), **different** (structurally distinct), **unmeasured**.

| # | Item | X11 file:line | Wayland file:line | Verdict | Action | Owner |
|---|---|---|---|---|---|---|
| 1 | Snap rect for 8 halves/quarters | `crates/flamewm-wm-x11/src/frame/controller/planning.rs:110-131` `snap_rect` | `crates/flamewm-window-core/src/lib.rs:117-138` `snap_geometry` (Wayland and X11 `wm/window_ops.rs:518` already call it) | **identical** (same `max(1)` half, `size - half` start) | Step B: delete `snap_rect`, call core | `flamewm-window-core` |
| 2 | Dock reserve cut of a work area | `crates/flamewm-wm-x11/src/wm/outputs.rs:8-13` `with_dock_reserve` | `crates/flamewm-wm-wayland/src/state.rs:1200-1210` `cut_dock` | **identical** (`None` unchanged, else `reserve_edge`) | Step C: one core fn `dock_reserve::cut_dock` | `flamewm-window-core` |
| 3 | Fit a restored rect back inside an area | `crates/flamewm-wm-x11/src/frame/geometry/bounds.rs:88-101` `fit_restore_rect` | `crates/flamewm-wm-wayland/src/model/geometry.rs:18-27` `clamp_into_output` | **near**: differ only when the rect extent is <= 0 (X11 min 1, Wayland min 0); Wayland also returns unchanged for an empty output | Step D: core `fit_into(rect, area, min_extent)`; wrappers keep their own rule | `flamewm-window-core` |
| 4 | Dock strut on work area (X11 fold over `dock_struts`) vs Wayland (`unmanaged` struts + layer zone) | `crates/flamewm-wm-x11/src/wm/outputs.rs:112-150` | `crates/flamewm-wm-wayland/src/state.rs:717-735` | **different** inputs (X11 raw dock struts with range checks; Wayland layer-shell zone). Both use `sanitize_strut` | not moved: input sources differ | - |
| 5 | Dock on a strut edge is not reserved twice | `crates/flamewm-wm-x11/src/wm/outputs.rs:19-31` `dock_reserve_uncovered_by_struts` | none | **X11-only**. Wayland has no equivalent check | risk: possible Wayland double reserve when a Dock publishes its own strut; not changed | - |
| 6 | Shortcut action-name parsers | `crates/flamewm-api/src/shortcuts.rs:71-112` (`workspace_activate_index`, `workspace_move_index`, `focus_direction`, `move_direction`, `workspace_direction`) | Wayland `input/shortcuts.rs` calls the same `flamewm_api` fns | **identical** (already shared) | none | `flamewm-api` |
| 6b | Shortcut dispatch bodies | X11 `wm/*` (`workspace.rs`, `window_ops.rs:373`) | Wayland `input/shortcuts.rs:14-60` `dispatch_action` | **different** (per-backend effects) | not moved | per backend |
| 6c | Media key mapping | none in X11 crates | `crates/flamewm-wm-wayland/src/input/mod.rs:52` `media_key` | Wayland-only, parses `flamewm_api` action names | not moved (no second user) | - |
| 7 | Keyboard tiling (Super+arrows) | none in X11 (no `ACTION_TILE_*` use under `crates/flamewm-wm-x11`) | `crates/flamewm-wm-wayland/src/input/keyboard_tile.rs` uses core `next_tile` | **different**: the module doc claims it matches X11, X11 has no tile handler | not moved; doc claim is wrong and should be corrected separately | - |
| 8 | Grab state machine (Move/Resize) | X11 `frame/controller*`, `frame/geometry/moves.rs` (plan_move, plan_resize, `MOVE_ACTIVATION_THRESHOLD_PX`) | `crates/flamewm-wm-wayland/src/state.rs:58` `enum Grab {Move, Resize}` | **different**: X11 is a reducer over frame events, Wayland a pointer-state enum with `anchor_num/den` | not moved in this pass; needs a designed pure-transition model | candidate `flamewm-window-core` |
| 9 | Keep-visible clamp | `crates/flamewm-wm-x11/src/frame/geometry/bounds.rs:10` delegates to core `keep_visible` | Wayland uses core too | **identical** (already shared) | none | `flamewm-window-core` |
| 10 | Cursor theme env | `crates/flamewm-session-core` `cursor_environment` (shared) | `crates/flamewm-wm-wayland/src/cursor.rs` | **identical** source (already shared) | none | `flamewm-session-core` |
| 11 | Workspace grid | `flamewm-api::workspace` | same | **identical** (already shared) | none | `flamewm-api` |
| 12 | Lock, focus/urgency, media on X11 | unmeasured (scans truncated) | `crates/flamewm-wm-wayland/src/session_lock.rs` | unmeasured | not in this pass | - |

## Steps and outcome

- Step A: this doc (audit).
- Step B: snap_rect to core snap_geometry (row 1).
- Step C: cut_dock to core (row 2).
- Step D: fit_into to core (row 3).

Rows 4, 5, 7, 8, 12 are not moved. Reasons are in the table.
