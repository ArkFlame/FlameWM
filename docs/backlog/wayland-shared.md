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

Sign inversion (meas-sign, resolved): python-xlib `win.translate_coords(root,0,0)` returns the NEGATED root origin. The correct call is `root.translate_coords(win,0,0)`. Checked against `xwininfo` Absolute upper-left on private Xvfb windows at (-40,-25) and (120,80). The self-test is `tools/test_flamewm_frame_origin.py` (`python3 -I -m unittest discover -s tools -p test_flamewm_frame_origin.py`, 3 tests). The harness `measure.py` is not tracked in the repo; its scratch copies (`/tmp/claude-1000/wl-frame/`, `wl-dock/`) carried the inverted call and were patched to the root-relative form. The frame values in the table above were read with the inverted sign and the table uses the on-screen origin, so they stand. Open item: `flamewm-wm` binaries used were the existing `/run/media/linsaftw/SDD/target-shared/fast` build, not rebuilt from 80c35d0d.

## Translucent titlebar over wallpaper, X11 vs Wayland (fixed: Wayland readback channel order)

Harness: `scripts/`-independent copy of the parity run (`run.sh`, `TRIGGER` unused for term), Xvfb `:290` (X11) and `:296` (Wayland), 1920x1080, terminal opened, `background_opacity = 80`, wallpaper `grad.png` (fill), accent off, dock off. Binaries built from this branch (`flamewm`, `flamewm-wayland`, `flamewm-terminal`, `flamewm-desktop`, `flamewm-shell`).

Measured at the flat titlebar pixel (x=900, one row inside the bar) with the wallpaper beneath it:

| Session | Bar y | Bar RGB (before fix) | Bar RGB (after fix) | Wallpaper under bar (expected) | Predicted `(1-a) * wallpaper`, a = 0.749 |
|---|---|---|---|---|---|
| X11 `flamewm` | 33-36 | 64,0,0 | 64,0,0 | about 250,0,5 | 63,0,1 |
| Wayland `flamewm-wayland` | 63-75 | 4,0,60 | 60,0,4 | about 240,0,15 | 60,0,4 |
| Wayland `flamewm-wayland` | 79-87 | 5,0,59 | 59,0,5 | about 236,0,19 | 59,0,5 |

- Root cause: the desktop capture is read back with `Fourcc::Abgr8888` (`render/backdrop.rs` `read_reduced`). On this stack the bytes come back blue-first: the top-left of a red wallpaper read as `[0,0,255,255]`. The blurred desktop therefore carried R and B swapped, and every translucent titlebar and dock blended the swapped colour. Body, launcher, selection and taskbar do not read this capture, so they matched.
- Fix: read back as `Fourcc::Argb8888` (BGRA_EXT). After the change the top-left reads `[255,0,0,255]`, and the backdrop used by `paint_background` is `[240,0,15,255]` at the sampled pixel.
- X11 was correct: its bar equals `(1-a) * wallpaper` with straight channel order. X11 unchanged.
- Window placement: the Wayland terminal sits 30 px lower than the X11 one in this harness (bar at y 60 vs 30). This is the placement difference already recorded above and is not part of this change.
- Unit guard: `translucent_bar_composites_known_wallpaper_in_rgb_order` (titlebar.rs) asserts `(4,0,60)` for black glass at alpha 191 over `(240,0,15)`. It covers the blend path only. The readback order is not unit-testable without GL, and is verified by the harness numbers above.
- Not verified: other GPUs or drivers. The same `Abgr8888` readback is used by the screencopy path in `render/mod.rs` (`render_rgba`) and was not measured here; it may show the same swap.

## Screencopy readback channel order (measured)

- Path: `flamewm-wm-wayland` `render/mod.rs` `offscreen_pixels` (`Abgr8888` create and copy) → `screencopy::serve` → `flamewm-screenshot --full` Wayland branch.
- Stack: origin/main `f9c1b491`, release build, nested `flamewm-wayland` (winit) on private Xvfb `:381` (`-noreset`), no desktop autostart, so the frame is the compositor clear colour `[0.07, 0.08, 0.11]`.
- Measured: `flamewm-screenshot --full --no-save --output` PNG (1350x641, RGB) top-left and centre pixels read `[18, 20, 28]`, which is R=18 G=20 B=28 in RGB order. A swapped channel order would read `[28, 20, 18]`. Not swapped.
- Conclusion: the screencopy path stays `Abgr8888`. Only the backdrop blur readback (`read_reduced`) needed `Argb8888`, per `f9c1b491`.
- Not measured: a red wallpaper sample; the check relies on the screenshot decoder's `Xrgb8888` to RGB conversion being correct.

## Row 12 source audit: lock, focus/urgency, media (X11 vs Wayland)

Scope: `wsh-lock-x11`. Method: source reads on `origin/main` `bddeff02`, no runtime run. Line refs are from that commit. Verdicts: **shared** (one owner, both call it), **duplicated** (same rule written twice), **differs-by-design** (input or mechanism differs on purpose), **unknown** (not established from source here).

### Lock

| Item | X11 evidence | Wayland evidence | Verdict |
|---|---|---|---|
| Lock trigger | `crates/flamewm-wm-x11/src/wm/events.rs:202`, `idle.rs:144`, `hot_corner.rs:157`, `lib.rs:758`, `flamewm-x11-desktop/src/ports.rs:214` all call `flamewm_x11_desktop::start_lock_screen` | `input/shortcuts.rs:160` `ACTION_SESSION_LOCK` only logs "session lock is a later phase"; no WM-initiated lock | differs-by-design (X11 spawns a lock client; Wayland waits for a client to request `ext-session-lock-v1`) |
| Lock process ownership | `crates/flamewm-x11-desktop/src/lock.rs:115-121` spawns `flamewm-lock` detached plus supervisor thread; fallback `loginctl lock-session` (`lock.rs:126`) | `session_lock.rs:122` `lock()` and `:135` `unlock()` handle the protocol; `lock_global.rs:15` `install` advertises the global; `lock_cover.rs` paints | differs-by-design |
| Crash supervision | `lock.rs:50` `decide`: respawn on non-success exit (SIGKILL drops grabs), `LOCK_RESPAWN_CAP` stop, `supervise_lock` `:72` | Protocol: a dead locker leaves the session locked (`session_lock.rs:37` `clear_lock_surfaces`, `:213` `prune_dead_locks`); no respawn; WM owns no lock process | differs-by-design |
| Input gating while locked | none found in X11 lock path (scan limited to `lock.rs`; `windows_reachable`-equivalent not located) | `session_lock.rs:201` `shortcuts_active`, `:207` `windows_reachable`, `:190` `screen_for`, `:174` `enter_lock` | unknown on X11 (not located); Wayland side is source-verified |
| Lock UI | `flamewm-lock` is a separate binary on both sides, X11 via `flamewm-lock/src/x11.rs`, Wayland via `flamewm-lock/src/wayland.rs` | same crate | shared (one UI crate, two backends) |

### Focus and urgency

| Item | X11 evidence | Wayland evidence | Verdict |
|---|---|---|---|
| New-map focus steal | `wm/focus_guard.rs:30` `map_may_take_focus(recent_input, has_active, transient)` | no equivalent in `wm-wayland` source; new toplevels are placed in `state.rs`/`model.rs` | differs-by-design (X11 input timestamp rule; Wayland has no timestamp rule for maps) |
| Active-window request policy | `focus_guard.rs:35` `active_request_may_focus(source, recent_input)`, pager source honoured after input | `protocols/compat.rs:30` `activation_allowed(has_input_serial, age)`, max age `:26` 10 s, `:100-118` `request_activation` | differs-by-design (inputs differ: X11 user-time/source vs Wayland token serial + age) |
| Focus set | `wm/window_ops.rs:4` `focus`, `:10` `focus_hover` (no restack), `:28` `focus_with_stacking` | `state.rs:817` `set_focus` (raises in model, `pin_focus_to_lock` `:855` short-circuits) | differs-by-design (restacking models differ) |
| Focus-follows-mouse setting | `live_settings.rs:271` `focus_follows_mouse` | `input/settings.rs:6` `saved_focus_follows_mouse`, `:12` `focus_follows_mouse_at` | duplicated (both read a boolean setting in their own backend; no shared reader found in the scan) |
| Urgency set on blocked activation | `wm/events.rs:750` sets `_NET_WM_STATE_DEMANDS_ATTENTION` via `update_demands_attention` `:662-680` | `compat.rs:106-108` drops a rejected activation silently; no urgency property or state in `wm-wayland` source (grep for urgen/demands_attention empty) | unknown on Wayland (feature absent in source; whether it should exist is not decided) |
| Urgency clear and read | cleared on focus `wm/window_ops.rs:39`, on map `manage.rs:593` sets, `client_state.rs:33` reads | none | X11-only |
| Super-tap focus loss | `wm/super_tap.rs:96` `focus_lost` | none found | unknown on Wayland |

### Media keys and control

| Item | X11 evidence | Wayland evidence | Verdict |
|---|---|---|---|
| Key names to actions | `flamewm-x11-desktop/src/shortcuts.rs:77-93` XF86 keysym table; actions from `flamewm_api` | `flamewm-platform/src/shortcuts.rs:383-399` defaults for the same `XF86Audio*` and `XF86MonBrightness*` names | shared (action ids and defaults from platform/api) |
| Action id to media key | `wm/events.rs:211-218` inline `match` (volume up/down/mute, brightness up/down) | `input/mod.rs:52-61` `media_key` (same five ids) | **duplicated** (same five-row mapping written twice; the Wayland one has a unit test, `input/tests.rs:25-45`; X11 has none found) |
| Audio apply | `runtime.rs:742-746` `host.media_audio_key(key, cap)` | `control/media.rs:64-68` `media_audio_key(key)` calls the same `host.media_audio_key(key, cap)` | shared (`flamewm-platform/src/system.rs:255-300` owns step, cap, mute and generation) |
| Brightness apply | `runtime.rs:748-749` `host.media_brightness_key(key)` | `control/media.rs:33-50` `media_brightness_key` calls the same host method | shared (platform owner) |
| Volume cap source | `live_settings.rs:51` `raise_maximum` snapshot, `runtime.rs:744` `volume_cap_percent()` | `control/media.rs:65` `volume_cap(&settings_snapshot())` | duplicated read of one shared setting; each backend resolves it separately |
| Missing control service | X11 path has no control-service gate (host called directly) | `input/shortcuts.rs:173-181` logs once and drops the key when `self.control` is `None` | differs-by-design (Wayland runs a separate control service) |
| MPRIS / media transport | X11 `runtime.rs:156` registers `LinuxIntegrationSource::Mpris` | not searched in this pass | unknown on Wayland |

### Summary

- Shared, no action: lock UI binary (`flamewm-lock`), media apply (`flamewm-platform` `system.rs`), shortcut names and defaults.
- Duplicated, candidate for a later move: the five-row action-to-media-key map (`wm/events.rs:211-218` vs `input/mod.rs:52-61`). Not moved in this pass. The map has no shared key enum today (audio and brightness are separate types in `flamewm-platform`), so moving it means adding a new type to a shared crate. That is a design step, not the tiny pure move this pass allowed. Also duplicated: the focus-follows-mouse bool reader and the volume-cap read.
- Differs by design: lock supervision, activation/focus-steal policy, restack model, missing-control-service path.
- Unknown: X11 lock input gating, Wayland urgency (absent in source), Wayland MPRIS, Super-tap focus loss on Wayland.
- Code change: none. Runtime check: none run, so no runtime claim is made. Row 12 in the table above stays "unmeasured" until a runtime pass.
