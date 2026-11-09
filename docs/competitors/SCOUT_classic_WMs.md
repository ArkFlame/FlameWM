# Scout: classic WMs and desktops (xfwm4, IceWM, MATE desktop, xfdesktop)

Snapshot: 2026-11-09, branch `main`. Read-only scout. No source edits, no Cargo builds, no commit.
Reference only: `.vendor/xfwm4-master`, `.vendor/icewm-master`, `.vendor/mate-desktop-master`, `.vendor/xfdesktop-master`.
Licences: xfwm4 and xfdesktop headers are GPL; MATE headers are GPL/LGPL; IceWM is LGPL upstream (grep did not confirm from its COPYING). **Reimplement from behaviour only. Copy no code.**
Constraint applied: no glass, blur, shadow or effect work. Rejected rows are marked as such.
Perf/mem column: qualitative estimates only. Nothing is measured (`unmeasured`).

## Table (23 rows)

Legend: FlameWM paths are relative to `crates/`. Verdict: ADOPT, ADAPT, or REJECT.

| # | Idea | Source (vendor) | FlameWM today | Gap | Effort | Perf/mem | Licence | Verdict and reason |
|---|---|---|---|---|---|---|---|---|
| 1 | Reject invalid struts (>= screen dim) and warn, so a buggy dock cannot reserve the whole screen | xfwm4 `src/netwm.c:999-1022` | `flamewm-wm-x11/src/wm/manage.rs:163-190` `read_strut` returns raw partial/legacy values, no bound check | No sanity bound; one bad dock can make the work area unusable | S | none | GPL | ADOPT. Reliability fix, tiny change, clear user win. |
| 2 | Keep orphaned windows visible when an output disappears or moves | xfwm4 `src/placement.c:571` `clientKeepVisible` | `flamewm-window-core/src/lib.rs:54` `keep_visible` exists (pure). Call site on output change not found by grep | Unverified that unplug/resize re-homes windows | S | none | GPL | ADOPT after verification. Pure helper exists; only wiring may be missing. Verify first. |
| 3 | Snap drag to neighbour window edges with a snap distance | xfwm4 `src/moveresize.c:369-410` `clientFindClosestEdgeX`, `:416` Y; IceWM `src/movesize.cc:18-40` `snapTo` | `flamewm-wm-x11/src/frame/model.rs:174` `SnapTarget`; `flamewm-wm-x11/src/wm/outputs.rs:227-237` `snap_zones_at` / `snap_target_at`. Screen zones only | No window-edge snapping, no snap distance | M | O(windows) per motion. Fits the existing motion-coalescing contract (`docs/ARCHITECTURE.md:71-74`) | GPL | ADAPT. Strong drag UX win. Use a candidate list built once per drag, not per event. |
| 4 | Edge resistance: a snap that releases only after a further push | xfwm4 `src/moveresize.c:507-590` (`snap_resist` checks) | none found | Snap is sticky-free; no resistance | S (after #3) | none | GPL | ADAPT as part of #3. Do not build alone. |
| 5 | Smart non-overlapping placement for new windows (first-fit free area) | xfwm4 `src/placement.c:634-816` `smartPlacement`; IceWM `src/wmmgr.cc:1480-1590` `getSmartPlace`, `:1562` `smartPlace` | `flamewm-wm-x11/src/frame/geometry/configure.rs:140` `plan_initial_map`, `:196` `plan_initial_map_near_pointer`. Default policy not verified | No free-space search for new windows | M | O(n*m) on map only; no steady-state cost | GPL / LGPL | ADAPT. Visible win on crowded desktops. Keep the pointer policy for pointer-launched windows. |
| 6 | Centre a transient over its parent, clamped to work area | xfwm4 `src/placement.c:573-598`; IceWM `src/wmmgr.cc:1665` | `flamewm-wm-x11/src/wm/transient_placement.rs:7-62` `centre_over_parent` + tests | Parity | - | - | - | REJECT. Already present and tested. |
| 7 | Auto-maximise windows that open larger than the screen | xfwm4 `src/placement.c:609` `clientAutoMaximize` | not found by grep | Unverified whether oversize windows are handled | S | none | GPL | ADAPT if confirmed missing. Verify before scheduling. |
| 8 | Route an unrequested focus steal to "demands attention" instead of focusing or ignoring | xfwm4 `src/client.c:960-970` (activate_action), `src/focus.c:144-181` | `flamewm-wm-x11/src/wm/focus_guard.rs:7` `RECENT_INPUT` (2 s), `:30` `map_may_take_focus`; `flamewm-wm-x11/src/wm/events.rs:646` `update_demands_attention` | Guard exists. Unclear whether a held activation sets the urgent flag | S | none | GPL | ADAPT. Small wiring on the existing guard. |
| 9 | Urgency blink with a repeat limit | xfwm4 `src/client.c:371-458` `urgent_cb` | flag set (`events.rs:646`), taskbar shows it (`crates/flamewm-shell-core/src/lib.rs:615`). No blink | Blinking is an animated effect | - | timer while urgent | GPL | REJECT. Blink is an effect, excluded by the user constraint. Keep the static urgent marker. |
| 10 | MRU focus ring used by Alt-Tab | xfwm4 `src/focus.c:401` `clientSortRing`, `:426` `clientSetLast`; `src/cycle.c:64-89` | `flamewm-wm-x11/src/wm/switcher.rs:37` `touch`, `:54` `switcher_order`; tests `:313-340` | Parity | - | - | - | REJECT. Already present and tested. |
| 11 | Edge-dwell workspace switch while dragging a window | xfwm4 `src/moveresize.c:642-770` (`wrap_resistance`, 250 ms) | `flamewm-wm-x11/src/wm/drag_edge.rs:6-12` (`DRAG_EDGE_DWELL_MS` 220, inset 24, cooldown 500); `hot_corner.rs` | Parity | - | - | GPL | REJECT. FlameWM already has dwell plus cooldown. |
| 12 | Workspace wrap-around for keyboard switching | xfwm4 `src/workspaces.c:150-205` `modify_with_wrap`, `:264` `wrap_cycle` | `docs/ARCHITECTURE.md:43-50`: no wrap by design (left/right in row, up/down switch rows) | Deliberate product rule | - | - | GPL | REJECT. Product decision, not a gap. Ask the user only if wrap is wanted. |
| 13 | Strut counted only on the primary monitor | xfwm4 `src/workspaces.c:572-575` | `flamewm-wm-x11/src/wm/outputs.rs:18` `dock_reserve_uncovered_by_struts` (per output) | FlameWM is already per-output | - | - | GPL | REJECT. xfwm's rule is a workaround. Keep the per-output model. |
| 14 | Work area recomputed on strut/property change | IceWM `src/wmmgr.cc:2495-2580` `updateWorkArea`, `:2763` `workAreaUpdated` | `flamewm-wm-x11/src/wm/mod.rs:615` `mark_work_area` | Parity | - | - | LGPL | REJECT. Already present. |
| 15 | Focus-delay timer before focus is applied | xfwm4 `src/focus.c:761-802` `delayed_focus_cb` | none | Not needed | - | adds timers and latency | GPL | REJECT. Adds latency and timers. FlameWM focus is immediate. |
| 16 | Save/restore WM session (SM / WM_SAVE_YOURSELF, layout across restarts) | xfwm4 `src/session.c`; IceWM `src/wmsession.cc` | none (`grep SAVE_YOURSELF` empty). Per-app geometry memory exists: `flamewm-window-core/src/geometry_memory.rs:1-12` | No layout restore across WM restart | L | low, but a new persistence surface | GPL / LGPL | ADAPT, deferred. High effort; geometry memory covers most of the value. |
| 17 | Debounced idle save of icon positions | xfdesktop `src/xfdesktop-icon-position-configs.c:504-519` `save_icons_idled` / `schedule_save` | `flamewm-window-core/src/geometry_memory.rs` (debounce, same pattern) | Parity for windows. Icon debounce unverified | - | - | GPL | REJECT. Same pattern already present. |
| 18 | Icon positions per monitor and per level, with migration | xfdesktop `src/xfdesktop-icon-position-configs.c:137-205`; `src/xfdesktop-icon-position-migration.c` | no icon-arrangement persistence found in `flamewm-desktop/src` | Unverified whether icon positions persist per output | M-L | low | GPL | ADAPT, after check. Key positions by output layout. Verify first. |
| 19 | "Arrange icons" and "Next background" in the desktop context menu | xfdesktop `src/xfdesktop-window-icon-manager.c:254-261` | `flamewm-desktop/src/menus.rs` exists. Items not verified | Unverified | S | none | GPL | ADAPT if missing. Two menu items on the existing menu. |
| 20 | Wallpaper cycling per workspace | xfdesktop `src/xfdesktop-backdrop-cycler.c`, `xfdesktop-backdrop-manager.c` | `flamewm-desktop/src/wallpaper_loader.rs` (modes default/custom/solid). No cycling found | No slideshow | M | periodic decode. Cost is real, so decode off-thread at a long interval | GPL | ADAPT. Visible and not an effect. Keep the interval long (>= 5 min). |
| 21 | Crossfade between wallpapers | MATE `libmate-desktop/mate-bg-crossfade.c` | none | Effect | - | per-frame blending | LGPL | REJECT. Crossfade is an effect, excluded by the user constraint. |
| 22 | Monitor profile matching, plus intended and backup config files, for safe apply | MATE `libmate-desktop/mate-rr-config.c:977` `mate_rr_config_applicable`, `:1013-1026` backup and intended filenames, `:1173` `ensure_primary`, `:1229` save | `flamewm-x11-desktop/src/topology.rs:117-143` `arrange` errors with "screen resize is not implemented" (`:143`) | No profile match, no last-known-good revert on failed apply | M | none at runtime | LGPL / GPL | ADAPT. Last-known-good revert is a reliability win for multi-monitor. Profile matching is optional. |
| 23 | Ensure a primary output exists after any layout change | MATE `libmate-desktop/mate-rr-config.c:1173` `mate_rr_config_ensure_primary` | not verified | Unverified | S | none | LGPL / GPL | ADOPT if missing. Cheap invariant. Verify first. |

Not checked: IceWM `wmtaskbar.cc` and `wmwinlist.cc` in depth; xfwm4 keyboard move and resize; MATE thumbnail cache failure markers; xfdesktop typeahead (`xfdesktop-icon-view.c:1774`); FlameWM output-change call sites beyond `wm/outputs.rs`; FlameWM desktop icon grid and keyboard navigation.

## Top 5 by user-visible win / effort

1. **Reject invalid struts** (row 1, S). Win: a bad dock can no longer make the screen unusable. Sketch: in `flamewm-wm-x11/src/wm/manage.rs` `read_strut` (`:163`), after reading `[u32; 12]`, zero any edge whose value is >= the screen dimension for that axis, and log a warning once per window. Add a unit test next to the existing tests in `flamewm-wm-x11/src/wm/margin_rehome_tests.rs`. Source behaviour: `netwm.c:999-1022`.

2. **Keep windows visible on output change** (row 2, S, verify first). Win: unplugging or moving a monitor no longer strands windows off-screen. Sketch: confirm where `set_output_work_areas` (`flamewm-wm-x11/src/wm/outputs.rs:158`) is called on topology change. If no caller applies `keep_visible` (`flamewm-window-core/src/lib.rs:54`) to managed frames, add one pass over managed frames after the work-area update, in `flamewm-wm-x11/src/wm/outputs.rs`. Add a test using the existing `keep_visible` cases (`flamewm-window-core/src/lib.rs:189`).

3. **Snap to neighbour windows with a distance** (row 3, M). Win: dragging a window next to another aligns edges, a common expectation. Sketch: in `flamewm-wm-x11/src/wm/outputs.rs`, extend `snap_zones_at` (`:227`) and `snap_target_at` (`:237`) so that, during a move drag, edges of other mapped windows on the same output within a snap distance also count. Build the candidate edge list once when the drag starts, in the drag state. Keep the coalesced motion path. Add a pure function and tests in `flamewm-wm-x11/src/frame/controller/tests_effects.rs` style. Add edge resistance (row 4) only after this works.

4. **Smart placement for new windows** (row 5, M). Win: new windows stop stacking on top of each other on crowded desktops. Sketch: in `flamewm-wm-x11/src/frame/geometry/configure.rs`, add a pure function `plan_smart_placement(work: RootRect, existing: &[RootRect], w, h) -> RootRect` that scans a coarse grid for the first free rectangle, falling back to cascade. Call it from `plan_initial_map` (`:140`) only when the pointer policy does not apply and the window is not transient. Keep `plan_initial_map_near_pointer` (`:196`) unchanged. Test it in `flamewm-wm-x11/src/frame/geometry/tests/`. Source: `placement.c:634-816`.

5. **Last-known-good monitor arrangement** (row 22, M). Win: a failed monitor change reverts instead of leaving a broken layout. Sketch: in `flamewm-x11-desktop/src/topology.rs` `arrange` (`:117`), save the previous placements before applying, and restore them on error. Replace the hard error at `:143` with a clean rejection that keeps the old layout. Do not add profile matching in this pass. Test with the existing topology tests. Source: `mate-rr-config.c:1013-1026` (backup concept only).

## Ranked honourable mentions

- Focus-steal routed to demands-attention (row 8, S).
- Icon positions per output (row 18, M-L).
- Wallpaper cycling (row 20, M).

## Checks run

- Grep and read only. No build, no test run, no edits to source. No commit.
- `scripts/version`, `.opencode/HARNESS.md`, `.opencode/ROUTES.md`, `docs/ARCHITECTURE.md`, `docs/competitors/README.md`: read.
- Not run: any Cargo command (scout scope). FlameWM behaviour claims above are static inspection only and need runtime proof before any change.
