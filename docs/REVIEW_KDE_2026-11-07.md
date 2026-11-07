# KDE vs FlameWM: techniques to adopt (2026-11-07)

Scope: read-only review. Sources: sparse shallow clones in `/tmp/claude-1000/kde-src/` (untrusted, not built or run):
- `kwin/`: `src/x11window.cpp`, `src/killwindow.cpp`, `src/placement.cpp`, `src/core/renderloop.cpp`, `src/scene/scene.cpp`, `src/workspace.cpp`
- `pw/` (plasma-workspace): `klipper/updateclipboardjob.cpp`, `libnotificationmanager/fullscreentracker.cpp`, `libnotificationmanager/`

Ours: grep evidence only, no runtime checks. "Ours: none" means no hit in the listed crates for the grep terms.

## Top 12 (ranked by user-visible reliability and compatibility win)

| # | Technique | KDE source | FlameWM today | Win | Effort | Risk |
|---|---|---|---|---|---|---|
| 1 | `_NET_WM_PING` two-stage hang detection: desaturate at timeout/2, then "force close?" prompt, then kill the process | `kwin/src/x11window.cpp:1440` `pingWindow()` (timer at `killPingTimeout()/2`, `setUnresponsive`), `:1490` `killProcess(ask)` | `NET_WM_PING` absent. `WM_DELETE_WINDOW` interned only (`crates/flamewm-wm-x11/src/lib.rs:127`, `atoms.rs:78`). No `xcb_kill_client` path | Hung apps get a recovery path instead of a frozen frame | M | Medium: kill must be gated on `_NET_WM_PID` and a local host check, plus a confirm UI in shell |
| 2 | Send `WM_TAKE_FOCUS` for Globally Active / no-input clients | `kwin/src/x11window.cpp:1637` `takeFocus()` | Atom interned (`atoms.rs:79,145`). No send path found | Focus works for Java, Qt, and Tk-style clients that ignore `XSetInputFocus` | S | Low |
| 3 | Focus-stealing prevention levels, with a per-app override and a `SessionState::Saving` exemption | `kwin/src/x11window.cpp:4157-4201` (`checkFSP`, `checkFPP`, `RelaxedForActive`) | `_NET_ACTIVE_WINDOW` used only as a request/response (`lib.rs:501,529`). No level policy found | Fewer surprise focus jumps; fewer "window ate my keyboard" reports | M | Medium: defaults must not block legit activation; start at Medium |
| 4 | Placement policies for new windows: Smart (`placeSmart`), Cascade (`placeCascaded`), Under-mouse, Centered for transients/dialogs, on-main-window | `kwin/src/placement.cpp:151` placeSmart, `:341` placeCentered, `:465` placeUnderMouse, `:559` cascade | `PlacementMode` has only Floating/Snapped/Maximized/Fullscreen (`crates/flamewm-wm-x11/src/frame/model.rs:185`). No new-window placement policy found | Dialogs open over their parent; no stacked-at-0,0 windows | M | Low |
| 5 | Clipboard survives owner exit (save targets to a clipboard manager on exit) | KDE Klipper keeps history: `pw/klipper/updateclipboardjob.cpp` (captures all MIME formats, skips >20 MB, hashes for dedupe) | `flamewm-clipboard/src/owner.rs` holds CLIPBOARD on a thread until `SelectionClear`. No `CLIPBOARD_MANAGER` or `SAVE_TARGETS` handling found | Copied text survives app exit and WM restart | M | Medium: owner thread must answer `SAVE_TARGETS` without blocking |
| 6 | Predictive frame scheduling: `expectedCompositingTime = renderJournal.result() + safetyMargin`, `presentNotBefore`, and early start after >100 idle pageflips (low-power GPU) | `kwin/src/core/renderloop.cpp:50-80` `scheduleRepaint()`, `kwin/src/scene/scene.cpp:199,261` (per-view `nextPresentationTimestamp`) | Wayland DRM is vblank-paced (`crates/flamewm-wm-wayland/src/drm/mod.rs:6,232`) with a first-frame deadline. No render-time prediction found. X11 presenter has no vblank scheduling found | Lower input-to-photon latency; no first-frame stall after idle | L | High: needs runtime timing evidence on hardware. Not proven |
| 7 | Auto-suppress notifications while a fullscreen window has focus, and while screen share is active | `pw/libnotificationmanager/fullscreentracker.cpp:12-28` `FullscreenTracker`, tied to the active task | DND toggle exists (`crates/flamewm-shell/src/notification_center.rs:61,101,137`). No fullscreen-triggered inhibition found | Notifications no longer pop over games and presentations | S | Low |
| 8 | Startup notification (`_NET_STARTUP_ID`, busy cursor until first map or timeout) | Not fetched (not in the sparse set) | None found (grep for `STARTUP_ID`, `_NET_STARTUP`, busy cursor: no WM hits) | Launch feedback; no "did my click work" gap | S-M | Low |
| 9 | Session management (SM/XSMP, xdg-session-management) with `SessionState::Saving` | `kwin/src/workspace.cpp` (session manager), `kwin/src/x11window.cpp:4169` (FSP exemption while saving) | Only logind `xdg_session_id` lookup found (`crates/flamewm-wm-wayland/src/logind/tests.rs:231`). No SM client found | Restore windows and positions after session logout and login | L | High: broad surface, needs a per-protocol design |
| 10 | Idle inhibit for X11 sessions (Wayland side exists) | Not fetched | Wayland: `crates/flamewm-wm-wayland/src/protocols/compat.rs:10,34` (`IdleInhibitHandler`). X11: none found | Screensaver and lock do not fire during video | M | Medium: X11 has no protocol. Needs a D-Bus or XScreenSaver path |
| 11 | Clipboard capture caps and hash dedupe (20 MB cap, SHA-1 per format) | `pw/klipper/updateclipboardjob.cpp:91-98` | Not checked in the capture path | Bounded memory on large image copies | S | Low |
| 12 | Urgency: `_NET_WM_STATE_DEMANDS_ATTENTION` handling | Not fetched in KWin's `x11window.cpp` pass | Atom present and mapped (`crates/flamewm-wm-x11/src/lib.rs:116,606`; `crates/flamewm-x11-desktop/src/shared.rs:65`). Behavior not verified | Taskbar and panel show attention reliably | S | Low. Confirm behavior before acting |

## Not covered (sparse set did not include them, or no time)

- KGlobalAccel conflict handling: not fetched.
- Virtual desktop edge cases, per-window rules, shaped/ARGB and input-only window handling: not fetched.
- Plasma hung-app dialog UI: only the KWin side (`killWindow`, ping) was read.

## Next actions

1. Item 2 (`WM_TAKE_FOCUS`): S, low risk. Start here.
2. Item 1 (ping and kill): M. Needs a shell prompt and an integration test with a hung `xterm`-style client.
3. Items 7 and 11: S each.

Git status of the repo was not changed. Only this file was written.

## Risks

- Runtime behavior of items 1, 3, and 6 is unproven. Each needs a live X11 or DRM run before claiming a fix.
- Only the sparse KWin and plasma-workspace paths above were read; other KDE code paths were not compared.
