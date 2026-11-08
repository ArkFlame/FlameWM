# Xfce (xfwm4, xfdesktop) learnings for FlameWM

Snapshot: 2026-11-08. Sources are read-only reference in `.vendor/xfwm4-master/` and `.vendor/xfdesktop-master/`.
Licence: both are GPL-2.0-or-later (per `COPYING`). Only ideas are used. No code is copied; any port is a Rust rewrite.
FlameWM paths are current `origin/main` (`478cc386`).

Verdict key: adopt (queued for integration), defer (valid, cost or conflict too high now), reject (no value or conflicts with product rules).
Effort: S < 1 day, M 1-3 days, L > 3 days. Memory/CPU: "none" means no idle cost and no per-event cost beyond the trigger.

## Headline

FlameWM already matches most xfwm4 behaviour: activation guard, press-focus, snap, struts, title double-click, placement, and most EWMH atoms. The real gaps are small and mostly UX policy, not X11 correctness. The clearest X11 gap is `_NET_WM_SYNC_REQUEST` (smooth resize), which is large. Nothing reviewed is a small, safe, zero-cost win that can be integrated without a settings or input-grab change, so no code was integrated in this pass.

## Learnings

| # | Idea | What xfwm4/xfdesktop does (vendor path:line) | FlameWM today (path:line) | Gap? | Effort | Mem/CPU | Verdict and reason |
|---|---|---|---|---|---|---|---|
| 1 | Focus-stealing prevention | `prevent_focus_stealing` and `activate_action` gate `_NET_ACTIVE_WINDOW` and new-window focus (`src/focus.c:144-171`, `src/netwm.c:1451-1453`, `src/client.c:970`, `src/client.c:2645-2672`) | Covered: `flamewm-wm-x11/src/wm/events.rs:716-735` uses `focus_guard::active_request_may_focus(data[0], recent_input)` and sets demands-attention when blocked | No (partial) | n/a | none | Already done. FlameWM does not use `_NET_WM_USER_TIME`/`_NET_STARTUP_ID`, so launches from non-input paths can still be blocked. Defer: adding user-time needs a new atom and timestamp plumbing. |
| 2 | Press focus and click-to-focus option | `click_to_focus` toggles focus-on-click vs pointer focus (`src/focus.c:366`, `src/focus.c:523`, `src/workspaces.c:370-386`) | Press-focus always on: `flamewm-wm-x11/src/wm/mod.rs:721-744` (`should_focus_on_press`, `press_focus_action`) | Option only | S | none | Reject. FlameWM has no pointer-focus mode to toggle away from. Adding a click-to-focus toggle adds settings with no user request. |
| 3 | Raise on click option | `raise_on_click` (`src/events.c:792`, `:952`, `:982`, `:1040`; `src/client.c:2187`) | Press path focuses and restacks, not separately configurable (`wm/mod.rs:721-744`) | Option only | S | none | Reject for now. Not a bug, and a toggle adds a setting. |
| 4 | Titlebar double-click action (maximize, shade, fill, hide, above) | `double_click_action` enum (`src/events.c:732-749`) | Maximize only, 500 ms and 4 px slop: `flamewm-wm-x11/src/frame/controller.rs:126-127`, `frame/controller/reducer.rs:265` | Yes (configurable action) | M | none | Defer. Value is real but needs a new setting through the settings owner (`flamewm-platform/src/settings.rs` is over 50 KB and must not be split). Revisit with the settings owner. |
| 5 | Alt+drag (easy_click) to move or resize from any point in the client | `easy_click` passive grab on the client plus Button1/2/3 with modifier (`src/client.c:354-357`, `src/events.c:605`, `:910-939`) | Not found: 0 matches for alt-drag or easy_click in `wm-x11`, `window-core`, `api` | Yes | M-L | Passive grabs on every managed client's buttons with modifier. Zero idle cost, but it steals Alt+click from apps. | Defer. Needs a grab-conflict design (apps that use Alt+drag) and a setting. Good candidate once settings are split. |
| 6 | Wrap-around workspaces | `wrap_workspaces` (`src/events.c:1437`, `src/moveresize.c:1288`) | Explicit no-wrap design: `flamewm-api/src/workspace.rs:186` and `docs/ARCHITECTURE.md` section 2 | Opposite of FlameWM rule | S | none | Reject. The architecture fixes a non-wrapping two-row grid. |
| 7 | Cycle through hidden windows in alt-tab | `cycle_hidden` (`src/cycle.c:72`) | Not found in `wm-x11`, `window-core`, `api` (alt-tab may live in the shell; not checked) | Unverified | S | none | Defer. Check the shell alt-tab owner before deciding. |
| 8 | Snap to screen border and to other windows | `snap_to_border`, `snap_to_windows` (`src/moveresize.c:388-548`, `:908`) | Covered: `flamewm-wm-x11/src/wm/snap_preview.rs`, `drag_edge.rs`, 12 files reference snap-to-windows | No | n/a | n/a | Already done. |
| 9 | Placement policy (transient centering, smart cascade) | Transient and splash placement (`src/client.c` placement code) | Covered: `flamewm-wm-x11/src/wm/manage.rs:88`, `:108`, `:239-266` (transient centre, `placement_floating`) | No | n/a | n/a | Already done. |
| 10 | Shade (window roll-up) | `_NET_WM_STATE_SHADED` plus shade action | Present: shade atoms in 2 files | No | n/a | n/a | Already done. |
| 11 | `_NET_WM_SYNC_REQUEST` (sync resize counter) | Sends `_NET_WM_SYNC_REQUEST` with counter for smooth resize (`src/xsync.c:239`, `src/events.c`) | Not found: 0 matches in all `crates/**/*.rs` | Yes (correctness/smoothness, GTK/Qt resize tearing) | L | Adds a counter per resized client. Only during interactive resize, so near zero idle cost. | Defer, highest-value X11 item. Touches resize path in `wm/configure*.rs` and `wm/events.rs`; needs runtime proof under Xvfb with a GTK client. |
| 12 | `_NET_REQUEST_FRAME_EXTENTS` handler | Replies by re-publishing `_NET_FRAME_EXTENTS` (`src/events.c:1939-1943`, `:1992-1996`) | Not handled as a message, but extents are published on manage and on every mode change: `wm/manage.rs:533`, `wm/client_state.rs:83-108`, `wm/effects.rs:86`, `window_ops.rs:574` | Mostly no | S | none | Reject as redundant. The property already holds the current value when a client asks. Adding a handler would be dead code. |
| 13 | `_NET_WM_USER_TIME` and `_NET_STARTUP_ID` | Uses user time to decide whether a window may take focus (`src/netwm.c:1451`, `src/hints.c`) | Not found in `wm-x11` (`_NET_STARTUP_ID` appears in 1 file, not wired to focus) | Yes | M | Stores one timestamp per client. Negligible. | Defer. It changes focus policy. Test first with a launcher on real apps. |
| 14 | `_NET_WM_PID` | Reads PID for hung-app handling (`src/events.c:1769`, `src/hints.c:1374`) | 1 reference in `crates/**` (not reviewed in full) | Unverified | S | none | Defer. Check the existing reference before adding anything. |
| 15 | `_NET_WM_OPAQUE_REGION`, `_NET_WM_WINDOW_OPACITY` | Compositor opacity and opaque hints (`src/display.c` atoms) | Not found, and FlameWM has no compositor in the X11 session | Only with a compositor | L | Compositor cost | Reject. Changing the blur/surface/outline look is forbidden by project rules, and there is no compositor path. |
| 16 | `_NET_WM_ICON_NAME`, `WM_CLIENT_MACHINE`, `WM_COLORMAP_WINDOWS` | Stored hints (`src/hints.c:386`) | Not found | Trivial | S | none | Reject. No consumer. |
| 17 | Per-workspace wallpaper (backdrop per workspace) | `xfdesktop-backdrop-manager.c`, `xfdesktop-backdrop-cycler.c` keep one backdrop per workspace and monitor | Not found: no per-workspace match in `flamewm-desktop` (`wallpaper_loader.rs`, `slots.rs`) | Yes (feature) | L | One decoded image per workspace and monitor. Must be LRU-bounded to protect memory. | Defer. Real feature gap, but it costs memory per workspace. Design a bounded cache first. |
| 18 | Desktop icon grid and arrange | `xfdesktop-icon-view.c`, `xfdesktop-icon-position-configs.c` | Covered by slot grid: `flamewm-desktop/src/slots.rs` | No (verify sort options) | n/a | n/a | Already done. Sort/arrange presets not compared. |
| 19 | xfconf-style settings (channel, property, live notify) | Typed settings with change notification (`xfconf`, not in vendor) | Typed `flamewm-settings-core` and `flamewm-platform/src/settings.rs` | No | n/a | n/a | Reject as a rewrite. The typed core is already the owner. Keep notify-on-change patterns, if any, in the existing owner. |

## Rejected and why (summary)

- Wrap-around workspaces: conflicts with the fixed two-row grid in `docs/ARCHITECTURE.md`.
- Compositor opacity and opaque region: no compositor, and visual changes are forbidden.
- Persistent or redundant handlers (frame-extents request, icon name, colormap windows): already covered or no consumer.

## Next actions (in order)

1. Smooth resize: implement `_NET_WM_SYNC_REQUEST` (row 11), with a Xvfb GTK client for runtime proof.
2. Alt-drag (row 5) and titlebar double-click action (row 4): both need a settings owner decision first.
3. Per-workspace wallpaper (row 17): needs a bounded-cache design before code.

Skipped or not checked: xfdesktop menus, xfdesktop icon-position migration, FlameWM's alt-tab owner (row 7), `_NET_WM_PID` use (row 14), and any runtime behaviour. Risk: rows marked "unverified" must be checked before a decision.
