# Window management, workspaces and multiple displays

Status vocabulary matches the [README feature table](../README.md#features):

- **Shipped**: present in current source and used in the screenshots or owner-tested runs.
- **Partial**: present with stated gaps.
- **Not implemented**: no code path found in source.
- **Unproven**: code exists, but no runtime evidence was captured in this document.

Every statement cites its source. Paths are relative to the repository root. Line numbers were
read at the time of writing; the file names are the stable reference.

Two backends exist: the X11 window manager (`crates/flamewm-wm-x11`, the default session) and the
Wayland compositor (`crates/flamewm-wm-wayland`, Smithay, native DRM/KMS or nested). Shared rules
live in `crates/flamewm-api` and `crates/flamewm-window-core`, and both backends call them.

## Window styles

### Floating, snap and maximize

| Behaviour | X11 | Wayland |
| --- | --- | --- |
| Floating move and resize | Shipped (README feature row "Reparenting, focus, minimize, maximize, fullscreen, move, resize") | Partial: move and resize grabs in `crates/flamewm-wm-wayland/src/input/pointer.rs`; runtime unproven |
| Snap previews while dragging | Shipped (README row "Snap previews"). Source: `crates/flamewm-wm-x11/src/frame/controller.rs`, `crates/flamewm-wm-x11/src/frame/model.rs:174` (`SnapTarget`) | Unproven: rounded preview in `crates/flamewm-wm-wayland/src/snap_preview.rs`, snap logic in `crates/flamewm-wm-wayland/src/input/snap.rs` |
| Snap targets | Shared enum `crates/flamewm-window-core/src/lib.rs:22-33`: left/right half, top/bottom half, four quarters, maximize | Same shared enum |
| Maximize | Shipped (README row). Maximize and restore paths are in `crates/flamewm-wm-x11/src/wm/keyboard_tile.rs` | Unproven. Minimize and maximize keys are unassigned by default (README keyboard table) |
| Edge drag to workspace | X11 source: `crates/flamewm-wm-x11/src/wm/drag_edge.rs` (220 ms dwell, 500 ms cooldown, 4 px hit zone, internal monitor boundaries ignored). Unproven at runtime | Not checked in this pass |

No source path auto-tiles windows: snapping and Super+arrow tiling act only on the window the user moves. Floating is the resting state (source inference from the absence of any layout code).

### Keyboard tiling

Super+arrow keyboard tiling is implemented on both backends. The transition function is shared:

- `crates/flamewm-window-core/src/keyboard_tile.rs` (`next_tile`, `Direction`) owns the state change.
- X11 applies it in `crates/flamewm-wm-x11/src/wm/keyboard_tile.rs` (`RESIZE_STEP` 24 px,
  `MIN_TILE_EXTENT` 120 px for shared-edge resizes).
- Wayland applies it in `crates/flamewm-wm-wayland/src/input/keyboard_tile.rs`.

Status: Shipped on X11 (source plus README snap/keyboard behaviour), Unproven on Wayland at runtime.

### Tiling: what does not exist

- **No automatic tiling layouts.** There is no master/stack, column or spiral layout. Searching the
  two backend crates for layout algorithms returns only the explicit halves/quarters above.
  Status: **Not implemented**.
- **No per-workspace layout setting.** Status: **Not implemented**.

### Popups, dialogs and transients

- **X11 transients (Shipped in source, Unproven at runtime).** `WM_TRANSIENT_FOR` is read in
  `crates/flamewm-wm-x11/src/wm/events.rs:655` and at manage time in
  `crates/flamewm-wm-x11/src/wm/manage.rs:254-266`. A transient is centred over its parent by
  `crates/flamewm-wm-x11/src/wm/transient_placement.rs` (`centre_over_parent`, clamped to the work
  area; oversized frames anchor to the top-left so controls stay reachable). A transient also
  inherits its parent's workspace (`manage.rs:266`). Test: `transient_placement.rs`
  (`dialog_is_centred_over_parent`).
- **Wayland transients (Not implemented).** A search of `crates/flamewm-wm-wayland/src` for
  `set_parent`, `SetParent` and `xdg` parent handling returned no match. Dialogs are placed as
  ordinary toplevels.
- **Stacking.** Not verified in this pass. Popups and menus across workspaces are Unproven.

### Focus

- **X11 focus-stealing prevention (Shipped in source).** `crates/flamewm-wm-x11/src/wm/focus_guard.rs`:
  a window the user did not ask for does not take focus just after user input. Pager and
  user-initiated activations always focus.
- **Wayland focus.** `crates/flamewm-wm-wayland/src/state/focus.rs`. Runtime Unproven.
- **Show desktop.** `crates/flamewm-wm-x11/src/wm/show_desktop.rs` iconifies visible normal windows
  and restores exactly that set. Wayland equivalent not checked in this pass.

### Shortcuts

Defaults come from `crates/flamewm-platform/src/shortcuts.rs` (lines 150-165 for workspace
arrows; line 203 onward for `Super+Shift+N`). The README keyboard table lists the same defaults.

## Workspaces

### Count and model

- **Count (Shipped on X11, Partial on Wayland).** X11 reads `config.workspaces`
  (`crates/flamewm-wm-x11/src/wm/construct.rs:48`, minimum 1). The platform stores the count in
  `workspaces.count` (`crates/flamewm-platform/src/services/workspaces.rs`). The maximum is 18
  (`MAX_WORKSPACES`, `crates/flamewm-api/src/workspace.rs:4`). The default value was not confirmed
  in this pass; check `services/workspaces.rs`.
- **Wayland model (Partial).** `crates/flamewm-wm-wayland/src/model.rs:196-200` clamps the count
  to `1..=MAX_WORKSPACES`; each window carries `workspace` (`model.rs:132`). Runtime unproven.
- **Grid.** Workspaces are laid out in two rows (`WORKSPACE_ROWS`, `crates/flamewm-api/src/workspace.rs:131`).
  `grid_neighbor` (`workspace.rs:189`) returns `None` at an edge, so the arrows do not wrap.

### Switching

| Input | X11 | Wayland |
| --- | --- | --- |
| Ctrl+Super+Left/Right/Up/Down | Shipped (shared grid, `flamewm-api`) | Partial: `crates/flamewm-wm-wayland/src/input/shortcuts.rs:20-24` (`workspace_step`); runtime unproven |
| Super+1 to Super+9 (activate) | Source: `crates/flamewm-platform/src/shortcuts.rs:164-` | Source: `input/shortcuts.rs:28-34` |
| Super+Shift+1 to Super+Shift+9 (move window) | Source: `shortcuts.rs:164-` and `crates/flamewm-wm-x11/src/wm/workspace.rs:265` (`move_to_workspace`) | Source: `input/shortcuts.rs:37-42` |
| Workspaces 10 to 18 | **No default key** (defaults only bind 1 to 9). Use Settings to bind them | Same |
| Edge-drag switching | Source only (`drag_edge.rs`), Unproven at runtime | Not checked |

Switching is global: `current_workspace` on X11 (`crates/flamewm-wm-x11/src/wm/workspace.rs:4-12`)
and `current` on Wayland (`model.rs:168`) are single fields. A switch hides and shows windows on
the whole session. On X11, focus is restored per workspace from `workspace_focus` (`workspace.rs`).

### Moving windows

- X11 moves a window by setting `_NET_WM_DESKTOP` (`crates/flamewm-wm-x11/src/lib.rs:695-704`) and
  remaps the desktop through a plan (`crates/flamewm-api/src/workspace.rs`, `remapped_desktop`).
- Wayland moves via `Model::move_to_workspace` (`crates/flamewm-wm-wayland/src/model.rs:330`).
- Status: Shipped in X11 source, Partial on Wayland. Runtime unproven on both.

### Sticky and panel

- **Sticky (Partial).** X11: `WindowState.sticky` (`crates/flamewm-wm-x11/src/client.rs:48`,
  visibility rule `client.rs:115`, `workspace.rs:66`). Wayland: the `sticky` flag appears in the
  control mirror (`crates/flamewm-wm-wayland/src/control/mirror.rs`) and in the switcher
  (`control/switcher.rs`), but no toplevel sets it from a user action. No default key binds it.
  Status: **Unproven** for users.
- **Panel and Dock.** The panel and Dock are shell clients. They reserve work area through struts
  (`crates/flamewm-wm-x11/src/wm/outputs.rs`, `crates/flamewm-wm-wayland/src/state/mod.rs`) and
  the shared Dock cut in `crates/flamewm-window-core/src/dock_reserve.rs`. Whether the panel is shown on every workspace was not verified in this pass.

## Multi-head (multiple displays)

### X11 (RandR)

- **Output discovery (Shipped in source).** `crates/flamewm-wm-x11/src/lib.rs:935-1000`
  enumerates RandR outputs and CRTCs. The primary output is read from `randr_get_output_primary`
  and is not inferred from enumeration order (`lib.rs:960-961`). A single output may use the
  root window as fallback (Xephyr).
- **Topology change (Partial).** A RandR topology flag is set by notifications and consumed once
  per runtime turn (`crates/flamewm-wm-x11/src/wm/outputs.rs:61`, `outputs.rs:171`).
  `crates/flamewm-wm-x11/src/blue_light.rs:129` re-applies gamma to the new CRTC set.
- **Removed output (Shipped in source, Unproven at runtime).** Windows on a removed output are
  rehomed. Source: `crates/flamewm-wm-x11/src/wm/mod.rs:671` (`rehome_outer`), tested in
  `crates/flamewm-wm-x11/src/wm/tests.rs:859` (`removed_output_rehomes_floating_snapped_and_maximized_clients`).
  Off-screen windows fall back to the largest overlap, then the primary output
  (`crates/flamewm-wm-x11/src/lib.rs:1200` test).
- **Per-output scale.** Not applied on X11. The displays backlog says so
  (`docs/backlog/displays.md`, "X11: per-output scale is not hooked there").
- **Workspaces across monitors.** One workspace set for the session, not one per monitor. A
  window on the second monitor follows the global workspace.

### Wayland

- **Output layout policy (Partial, unit-level).** `crates/flamewm-wm-wayland/src/output_layout.rs`
  holds mode selection, fractional scale math, `arrange_row`, `output_at` and
  `clamp_point_to_layout`. Only mode selection is called from the live DRM path. The row layout
  and crossing helpers have no non-test callers (`docs/backlog/displays.md`).
- **Second output placement.** `origin_after` (`output_layout.rs:120`) gives each DRM output after
  the first a position to the right of the previous one, used by `drm/outputs.rs::build_output`.
  Status: Partial. Runtime proof is absent because the nested backend exposes one output.
- **Per-output scale (Partial).** Scale is persisted per output id (`crates/flamewm-platform/src/settings/product.rs:255`,
  `per_output_scale`) and applied to the output the compositor drives (`main.rs`
  `presentation_scale::output_scale`, `state/mod.rs`). The README says index 0 is the only output
  the platform snapshot reports today, so scale for index > 0 is Unproven.
- **Hotplug (Partial).** `crates/flamewm-wm-wayland/src/drm/hotplug.rs`: udev change events for the
  primary GPU are debounced (`HOTPLUG_SETTLE` 250 ms). `crates/flamewm-wm-wayland/src/drm/init.rs:196-230`
  rescans connectors once the burst is quiet, then calls `state.reflow_if_work_area_changed()` so
  off-screen windows are pulled back. Runtime Unproven.
- **Clamping after shrink.** `crates/flamewm-wm-wayland/src/model/tests.rs:1245`
  (`clamp_floating_into_pulls_offscreen_windows_back_after_an_output_shrink`). Unit test only.
- **Session lock per output.** `crates/flamewm-wm-wayland/src/lock_cover.rs` tracks coverage per
  connector. A lock covers the session only when every output has a live lock surface. The README
  says second-output lock surfaces are not rendered yet, so coverage is Partial and the desktop
  stays unreachable on uncovered outputs (`session_lock::windows_reachable`).

### Workspaces times monitors (the intersection)

| Question | X11 | Wayland |
| --- | --- | --- |
| One workspace per monitor? | No. One global current workspace | No. `current` is global (`model.rs:168`) |
| Windows on second monitor when switching? | Follow the global workspace (Unproven at runtime) | Follow the global workspace (Unproven) |
| Move window to another monitor? | Not verified in this pass | Not verified in this pass |
| New output appears? | Work area recomputed on RandR topology change (Partial) | Connectors rescanned, origin from `origin_after` (Partial) |
| Output removed? | Windows rehomed (Shipped in source, Unproven runtime) | Windows pulled back via reflow (Partial, Unproven) |
| Per-output scale? | Not applied | Applied to the driven output only (Partial) |
| Panel per output? | Not verified in this pass | Not verified in this pass |

## Added in the gap-closing pass (2026-11-19)

Each item below compiles and has unit tests (`cargo test` for `flamewm-api`, `flamewm-window-core`,
`flamewm-platform`, `flamewm-wm-x11`, `flamewm-wm-wayland`, all green). "Wired" means reachable from a
running session; "Library" means a tested module that no backend calls yet.

| Item | Path | Status |
| --- | --- | --- |
| Move window to next/prev monitor, focus next/prev monitor (X11) | `crates/flamewm-wm-x11/src/wm/monitor_actions.rs`, `crates/flamewm-window-core/src/monitor.rs`; keys Super+Shift+Alt+Left/Right and Super+Ctrl+Shift+Left/Right in `flamewm-platform/src/shortcuts.rs` | Wired in X11 dispatch (`wm/events.rs`); runtime Unproven; not on Wayland |
| Wayland `xdg_toplevel.set_parent` placement | `crates/flamewm-wm-wayland/src/transient.rs`, `handlers.rs` | Wired; runtime Unproven |
| Wayland lock surface per output | `lock_cover.rs`, `session_lock.rs`, `render/mod.rs` | Wired; runtime Unproven |
| Virtual outputs in the nested backend (`FLAMEWM_VIRTUAL_OUTPUTS=N`) | `crates/flamewm-wm-wayland/src/virtual_output.rs` | Wired; outputs are protocol and layout only, not painted |
| Window rules and monitor rules (`[[window_rule]]`, `[[monitor_rule]]`) | `crates/flamewm-api/src/rules.rs` | Library, not read from settings yet |
| Tiling layouts (master-stack, columns, monocle, dwindle) | `crates/flamewm-window-core/src/tiling.rs` | Library, no backend uses it |
| Opt-in per-output workspaces | `crates/flamewm-window-core/src/workspaces_per_output.rs` | Library, backends still use the global model |
| X11 per-output scale (RandR transform plan and apply) | `crates/flamewm-wm-x11/src/output_scale.rs` | Library, not called from the topology path |

The "No automatic tiling layouts", "per-monitor workspaces" and "per-output scale on X11" caveats above
therefore still hold at runtime until the Library items are wired.

## Known caveats

- **Multi-output placement is not applied by the Wayland compositor** (README, displays row).
  Only one output is driven, so multi-head Wayland is effectively single-output. Status: Partial.
- **Per-output scale is not applied on X11.** Status: Not implemented.
- **Workspaces are not per-monitor on either backend.** This is a design fact of the current model,
  not a bug. Status: Shipped as global.
- **No automatic tiling layouts.** Status: Not implemented.
- **Wayland has no transient placement.** Status: Not implemented.
- **Workspaces 10 to 18 have no default key.** Status: Shipped as a gap, not a bug.
- **Wayland native clients are SHM only**, and the shell, desktop and settings run as XWayland
  clients (README, Wayland known limitations). Affects how multi-head and scale are seen by apps.
- **Session lock on Wayland covers only the driven output** (README). X11 lock coverage on a second
  monitor is Unproven.
- **Runtime proof for all multi-head statements is absent.** The nested winit backend and the
  Xephyr fallback expose one output. A headless or virtual multi-output path was not identified
  (`docs/backlog/displays.md`, item 4).

## How to verify

Run these from the repository root. Each one is a read or a unit test; none starts a live session. None of these commands was executed while writing this document; the test names are taken from source.

```sh
# Source citations used in this document still exist
grep -n "centre_over_parent" crates/flamewm-wm-x11/src/wm/transient_placement.rs
grep -n "origin_after" crates/flamewm-wm-wayland/src/output_layout.rs
grep -n "MAX_WORKSPACES" crates/flamewm-api/src/workspace.rs

# Unit tests behind the claims (X11 and Wayland)
cargo test -p flamewm-wm-x11 transient
cargo test -p flamewm-wm-x11 removed_output_rehomes
cargo test -p flamewm-wm-wayland output_layout
cargo test -p flamewm-wm-wayland clamp_floating_into_pulls_offscreen

# Runtime checks (need a session; these are the Unproven items). Not run in this pass.
# X11, nested in Xephyr with two outputs (RandR), then:
xrandr --listmonitors
# Wayland nested backend, then check the log for applied scale and origin per output.
```

A claim is Shipped only when its unit test passes and a runtime run was captured. The unit
tests above show source behaviour, not a live session.

## Evidence index

| Claim | Source |
| --- | --- |
| Snap targets (halves, quarters, maximize) | `crates/flamewm-window-core/src/lib.rs:22-33` |
| Keyboard tiling, X11 | `crates/flamewm-wm-x11/src/wm/keyboard_tile.rs`, `crates/flamewm-window-core/src/keyboard_tile.rs` |
| Keyboard tiling, Wayland | `crates/flamewm-wm-wayland/src/input/keyboard_tile.rs` |
| Snap preview, Wayland | `crates/flamewm-wm-wayland/src/snap_preview.rs`, `crates/flamewm-wm-wayland/src/input/snap.rs` |
| Edge-drag switching | `crates/flamewm-wm-x11/src/wm/drag_edge.rs` |
| Transient placement | `crates/flamewm-wm-x11/src/wm/transient_placement.rs`, `crates/flamewm-wm-x11/src/wm/manage.rs:254-266`, `crates/flamewm-wm-x11/src/wm/events.rs:655` |
| Focus stealing prevention | `crates/flamewm-wm-x11/src/wm/focus_guard.rs` |
| Show desktop | `crates/flamewm-wm-x11/src/wm/show_desktop.rs` |
| Workspace count and max | `crates/flamewm-api/src/workspace.rs:4`, `crates/flamewm-wm-x11/src/wm/construct.rs:48`, `crates/flamewm-platform/src/services/workspaces.rs` |
| Workspace grid | `crates/flamewm-api/src/workspace.rs:131-200` |
| X11 switching | `crates/flamewm-wm-x11/src/wm/workspace.rs` |
| Wayland switching | `crates/flamewm-wm-wayland/src/model.rs:168,196-200,322-343,622-628`, `crates/flamewm-wm-wayland/src/input/shortcuts.rs:20-42` |
| Default shortcuts | `crates/flamewm-platform/src/shortcuts.rs:150-165,203` |
| Moving to workspace (X11) | `crates/flamewm-wm-x11/src/lib.rs:695-704`, `crates/flamewm-wm-x11/src/wm/workspace.rs:265` |
| Sticky | `crates/flamewm-wm-x11/src/client.rs:48,115`, `crates/flamewm-wm-x11/src/wm/workspace.rs:66`, `crates/flamewm-wm-wayland/src/control/mirror.rs` |
| X11 RandR outputs and primary | `crates/flamewm-wm-x11/src/lib.rs:935-1000` |
| X11 RandR topology flag | `crates/flamewm-wm-x11/src/wm/outputs.rs:61,171` |
| Removed output rehome | `crates/flamewm-wm-x11/src/wm/mod.rs:671`, `crates/flamewm-wm-x11/src/wm/tests.rs:859` |
| Wayland output layout | `crates/flamewm-wm-wayland/src/output_layout.rs` |
| Wayland hotplug | `crates/flamewm-wm-wayland/src/drm/hotplug.rs`, `crates/flamewm-wm-wayland/src/drm/init.rs:196-230` |
| Wayland lock coverage | `crates/flamewm-wm-wayland/src/lock_cover.rs`, `crates/flamewm-wm-wayland/src/session_lock.rs` |
| Per-output scale persistence | `crates/flamewm-platform/src/settings/product.rs:255` |
| Multi-output status | `README.md` (Displays row), `docs/backlog/displays.md` |
