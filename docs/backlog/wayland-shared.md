# Wayland/X11 shared-logic audit

> Rows: `wsh-dock-reserve` (done `79aa37c7`), `wsh-grab`, `wsh-strut`, `wsh-lock-x11` in `docs/BACKLOG.md`.

Base: `origin/main` at worktree creation. Method: function-level reads of both sides (not name or size
judgement). Verdicts: **identical** (same output for every input), **near** (same output for the
reachable inputs, differing edge case), **different** (structurally distinct), **unmeasured**.

| # | Item | X11 file:line | Wayland file:line | Verdict | Action | Owner |
|---|---|---|---|---|---|---|
| 1 | Snap rect for 8 halves/quarters | `crates/flamewm-wm-x11/src/frame/controller/planning.rs:110-131` `snap_rect` | `crates/flamewm-window-core/src/lib.rs:117-138` `snap_geometry` (Wayland and X11 `wm/window_ops.rs:518` already call it) | **identical** (same `max(1)` half, `size - half` start) | Step B: delete `snap_rect`, call core | `flamewm-window-core` |
| 2 | Dock reserve cut of a work area | `crates/flamewm-wm-x11/src/wm/outputs.rs:8-13` `with_dock_reserve` | `crates/flamewm-wm-wayland/src/state.rs:1200-1210` `cut_dock` | **identical** (`None` unchanged, else `reserve_edge`) | Step C: one core fn `dock_reserve::cut_dock` | `flamewm-window-core` |
| 3 | Fit a restored rect back inside an area | `crates/flamewm-wm-x11/src/frame/geometry/bounds.rs:88-101` `fit_restore_rect` | `crates/flamewm-wm-wayland/src/model/geometry.rs:18-27` `clamp_into_output` | **near**: differ only when the rect extent is <= 0 (X11 min 1, Wayland min 0); Wayland also returns unchanged for an empty output | Step D: core `fit_into(rect, area, min_extent)`; wrappers keep their own rule | `flamewm-window-core` |
| 4 | Dock strut on work area (X11 fold over `dock_struts`) vs Wayland (`unmanaged` struts + layer zone) | `crates/flamewm-wm-x11/src/wm/outputs.rs:112-150` | `crates/flamewm-wm-wayland/src/state.rs:717-735` | **different** inputs (X11 raw dock struts with range checks; Wayland layer-shell zone). Both use `sanitize_strut` | not moved: input sources differ | - |
| 5 | Dock on a strut edge is not reserved twice | `crates/flamewm-window-core/src/dock_reserve.rs` `dock_reserve_uncovered_by_struts` (was X11 `wm/outputs.rs`) | `crates/flamewm-wm-wayland/src/state.rs` `work_area_with_dock` | **was different, now identical**. Runtime proved a Wayland double reserve (before: 116 px for one 58 px Dock) | Step E: shared core fn, Wayland `work_area` applies it to the folded unmanaged struts | `flamewm-window-core` |
| 6 | Shortcut action-name parsers | `crates/flamewm-api/src/shortcuts.rs:71-112` (`workspace_activate_index`, `workspace_move_index`, `focus_direction`, `move_direction`, `workspace_direction`) | Wayland `input/shortcuts.rs` calls the same `flamewm_api` fns | **identical** (already shared) | none | `flamewm-api` |
| 6b | Shortcut dispatch bodies | X11 `wm/*` (`workspace.rs`, `window_ops.rs:373`) | Wayland `input/shortcuts.rs:14-60` `dispatch_action` | **different** (per-backend effects) | not moved | per backend |
| 6c | Media key mapping | none in X11 crates | `crates/flamewm-wm-wayland/src/input/mod.rs:52` `media_key` | Wayland-only, parses `flamewm_api` action names | not moved (no second user) | - |
| 7 | Keyboard tiling (Super+arrows) | `crates/flamewm-wm-x11/src/wm/keyboard_tile.rs` (core `next_tile`, `tile_state` now `SnapTarget::core()`) | `crates/flamewm-wm-wayland/src/input/keyboard_tile.rs` (core `next_tile`) | **identical transition**: both call core `next_tile`; the tile-state mapping was duplicated and is now single-owner on X11 | done in Step B (X11 side); Wayland `tile_state` still a local match, same fix possible | `flamewm-window-core` |
| 8 | Grab state machine (Move/Resize) | X11 `frame/controller*`, `frame/geometry/moves.rs` (plan_move, plan_resize, `MOVE_ACTIVATION_THRESHOLD_PX`) | `crates/flamewm-wm-wayland/src/state.rs:58` `enum Grab {Move, Resize}` | **different**: X11 is a reducer over frame events, Wayland a pointer-state enum with `anchor_num/den` | not moved. Probed on base `8831737a`: no shared move/resize fn. Move: X11 `plan_move` = start + delta then `keep_on_outputs` and titlebar clamp; Wayland `Grab::Move` = pointer anchor (`anchor_num/anchor_den`, `anchor_y`), so different inputs. Resize: Wayland `ResizeEdge::resized` (`model/geometry.rs (line unpinned)`) is a per-axis fixed-opposite-edge length with min clamp, origin follows, unmoved axis clamped to `MIN_CLIENT_W/H`; X11 `plan_resize` (`frame/geometry/moves.rs`) clamps moving edges to output bounds, caps to work area, applies `snap_client_size` hints, unmoved axis `max(1)`. Identical overlap is that Wayland per-axis math only (<30 lines, X11 does not run it), so a shared fn would not reduce net lines. Next step only with a designed transition model | candidate `flamewm-window-core` |
| 9 | Keep-visible clamp | `crates/flamewm-wm-x11/src/frame/geometry/bounds.rs:10` delegates to core `keep_visible` | Wayland uses core too | **identical** (already shared) | none | `flamewm-window-core` |
| 10 | Cursor theme env | `crates/flamewm-session-core` `cursor_environment` (shared) | `crates/flamewm-wm-wayland/src/cursor.rs` | **identical** source (already shared) | none | `flamewm-session-core` |
| 11 | Workspace grid | `flamewm-api::workspace` | same | **identical** (already shared) | none | `flamewm-api` |
| 12 | Lock, focus/urgency, media on X11 | unmeasured (scans truncated) | `crates/flamewm-wm-wayland/src/session_lock.rs` | unmeasured | not in this pass | - |

## Steps and outcome (landed on main)

- Step A `204d0281`: this audit doc.
- Step B `1822dbcc`: X11 `snap_rect` delegates to core `snap_geometry`; `SnapTarget::core()` replaces the X11 tile-state match. Gate on rebased tree: EXIT_GUARD=0 EXIT_FMT=0 EXIT_CLIPPY=0 EXIT_TEST=0 GATE_DONE all=0. Deletions: 28 lines.
- Step C `fef5f545`: `dock_reserve::cut_dock` shared by X11 `with_dock_reserve` and Wayland `cut_dock`. Gate: all 0. Deletions: 12 lines.
- Step D `40aaa56d`: `fit_into(rect, area, min_extent)` shared by X11 `fit_restore_rect` (min 1) and Wayland `clamp_into_output` (min 0, empty-output guard kept). Gate: all 0. Deletions: 17 lines.

- Step E (this commit): row 5 double reserve, confirmed at runtime and fixed. Method: `flamewm-wm` (X11) and `flamewm-wm-wayland` built with `--profile fast`, one private Xvfb 1920x1080 per session, same output size, `flamewm-desktop` and `flamewm-shell` running, one xterm maximized by `_NET_WM_STATE` add, client rect read with `xwininfo`-equivalent X queries. Harness: `/tmp/claude-1000/wl-dock/run.sh <x11|wl> <0|1>` (not committed).
  - Before the fix (Dock off -> on, same session): X11 frame height 1032 -> 974 (-58 = `dock_extent(32)`, both reps). Wayland client height 1001 -> 885 (-116 = 2 x 58, rep 1; rep 2 Dock-on 885).
  - After the fix: X11 unchanged (1000 -> 942, -58). Wayland 1001 -> 943 (-58), rep 1 and rep 2 Dock-on 943; Dock-off 1001 (rep 1 and try 1, try 2) or 1045 (one run with the taskbar not yet arranged; harness race, not a reserve result).
  - Visual check: `run-wl-dock1/shot.png` shows the Dock on the bottom edge with the xterm ending above it.
  - Harness flakes seen: some Wayland startups aborted (`Failed to initialize an event loop`), and some runs found no shell child within 12 s. Those runs were discarded; only completed measurements are reported above.

- Step E runtime re-verify on final main `79aa37c7` (fast build; private Xvfb `:151` X11 and `:152` Wayland host, 1920x1080; one xterm maximized per run; taskbar on in every run). Harness copy: `/tmp/claude-1000/wt-dock2-run/run2.sh <x11|wl> <0|1>` with `POS=bottom|left|right|top AH=0|1` (not committed). Client rect, W x H (x,y):

| Mode | X11 client | Δ X11 | Wayland client | Δ Wayland |
|---|---|---|---|---|
| Dock off (taskbar only) | 1914 x 1000 (-3,-33) | - | 1916 x 1001 (-2,-33) | - |
| Dock bottom (taskbar + Dock) | 1914 x 942 (-3,-33) | h -58 | 1916 x 943 (-2,-33) | h -58 |
| Dock left | 1856 x 1000 (-61,-33) | w -58 | 1858 x 1001 (-60,-33) | w -58 |
| Dock right | 1856 x 1000 (-3,-33) | w -58 | 1858 x 1001 (-2,-33) | w -58 |
| Dock top | 1914 x 942 (-3,-91) | h -58, y -58 | 1916 x 943 (-2,-91) | h -58, y -58 |
| Dock bottom, auto-hide on | 1914 x 1000 (-3,-33) | 0 | 1916 x 1001 (-2,-33) | 0 |

  - Result: every edge removes exactly 58 px (`dock_extent(32)`) in both sessions; auto-hide reserves nothing in both. No edge/mode differs by more than 1 px in its Dock delta, so no code change.
  - Absolute differences (X11 client 2 px narrower and 1 px shorter, left x 1 px more negative) also appear with Dock off, so they are frame-decoration baseline, not Dock reserve. Not investigated further.
  - Not run: the gate (no code change in this step), and any taskbar-off mode (no key used).

Rows 4, 6b, 6c, 8, 12 are not moved; reasons are in the table. Row 8 (Grab state machine) is the largest remaining item.

## Frame baseline X11 vs Wayland (decided: intended, no code change)

Maximized xterm, Dock off, measured in both sessions. The harness printed signed `translate_coords` values, so the sign of the reported x/y is inverted; the table uses the on-screen origin.

| Session | Frame (x,y,w,h) | Client (x,y,w,h) | Visible content (pixel scan) |
|---|---|---|---|
| X11 `flamewm-wm` | 2,2,1916,1032 | 3,33,1914,1000 | rim `#cccccc` at x=2 and x=1917, bottom y=1033; white from x=3 to 1916, y=33 to 1032 |
| Wayland `flamewm-wm-wayland` | 2,2,1916,1032 | 2,33,1916,1001 | same rim pixels; the rim overlays client column x=2, row y=1033 and column x=1917 |

Floating xterm, Dock off (screenshots `/tmp/claude-1000/wl-frame/run-*-dock0/shot.png`):
- X11: frame 200,170,486x348; client 201,201,484x316. Rim 1 px `#cccccc` at x=200 and x=685; titlebar y=170..200 (31 px); client white from x=201.
- Wayland: frame 60,60,484x347; client 60,91,484x316. Rim 1 px `#cccccc` at x=60 and x=543, overlaying client column 60; titlebar y=60..90 (31 px); client from y=91.

Cause: the X11 frame-band path (no compositing manager, `outline_overlay_available == false`) subtracts the outline band (`windowOutlineWidth` = 1, `surface_outline_width`) from the client on left, right and bottom (`FrameExtents::with_inset`, `wm/window_state.rs:7`). The compositing-manager overlay path keeps the full client rect (`wm/window_state.rs:17`, test `frame_extents_publish_tests.rs:177`). Wayland always uses the overlay path: `render/titlebar.rs:567` `outline_elements` draws the ring on top of the client, and `model/geometry.rs (line unpinned)` `client_from_frame` subtracts only the titlebar. The shared rim rule is `flamewm-render-core/src/outline.rs` (band at inset 0 of the silhouette), so visible pixels match.

Decision: intended mechanism difference, not a shared-geometry bug. The 2 px width and 1 px height gap is the band that X11 reserves inside the client; Wayland paints it over the client. Titlebar 31 px, rim 1 px, frame outer rect identical in both. No code change. Re-open only if one backend adopts the other's band model.

Open items (not investigated): the harness sign inversion in `measure.py` (`win.translate_coords(root,0,0)` returns negated coordinates); `flamewm-wm` binaries used were the existing `/run/media/linsaftw/SDD/target-shared/fast` build, not rebuilt from 80c35d0d.
