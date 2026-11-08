# KDE Plasma learnings for FlameWM

Snapshot: 2026-11-08. Sources: read-only scratch clones of `kwin`, `plasma-desktop`, `plasma-workspace` (see `SOURCE_MAP.md`).
Licence: GPL-2.0 or later and LGPL per repo `LICENSES/`. Only ideas are used. No code is copied; any port is a Rust rewrite.
FlameWM paths are `origin/main` at this snapshot.

Verdict key: `adopt` (in `WINNERS.md`), `defer` (valid, cost or conflict too high now), `reject` (no value, duplicate, or conflicts with project rules), `covered` (FlameWM already has it).
Effort: S < 1 day, M 1-3 days, L > 3 days. Memory: "none" means no idle cost.

## Headline

FlameWM already covers most of KDE's shell surface: launcher (finder), night light, edge snap, keyboard tiling, hot-corner overview, DND, tray, screenshot, and lock. The real gaps are small: clipboard history, sticky keys, shake-to-locate, and a workspace-switch OSD. The first three can be bounded to near-zero idle memory. Excluded from this pass: dock, weather tint, icon-theme and taskbar alignment, accent token, notification history bounds, DND settings toggle, RenderBackend enum.

## Learnings

| # | Idea | What KDE does (path) | FlameWM today (path) | Gap? | Effort | Mem/CPU | Verdict and reason |
|---|---|---|---|---|---|---|---|
| 1 | Clipboard history with bounded size | Klipper keeps a history model, default 20 items, max 2048 (`plasma-workspace/klipper/klipper.kcfg:54-58`, `historymodel.cpp`) | Keeper retains only the last text, capped at 1 MiB (`crates/flamewm-clipboard/src/keeper.rs:21`) | Yes | S-M | ~0 idle; bounded when full | adopt (`WINNERS.md` rank 1). Bounded by items and bytes. |
| 2 | Password-manager exclusion | Skip owners advertising `x-kde-passwordManagerHint` (`klipper/historymodel.cpp`) | Covered in keeper: `PASSWORD_HINT` never kept (`keeper.rs:24`) | No | n/a | none | covered. Reuse the same check in rank 1. |
| 3 | Sticky keys (latch modifiers) | KWin `stickykeys/` plugin; KCM `plasma-desktop/kcms/keys/` | Not found: 0 `.rs` hits for sticky or slow keys in `crates/` | Yes | M | none (state machine only) | adopt (`WINNERS.md` rank 2). Accessibility gap, zero idle cost. |
| 4 | Shake pointer to locate | KWin `shakecursor/shakedetector.cpp` | Not found: 0 `.rs` hits | Yes | S | none (event-driven ring) | adopt (`WINNERS.md` rank 3). |
| 5 | Workspace switch OSD | KWin `desktopchangeosd/` (transient, auto-hides) | Not found: no workspace OSD (other OSDs are assistant, backlight, audio) | Yes | M | ~0 if destroyed after timeout | adopt conditional (`WINNERS.md` rank 4). Owner not yet mapped. |
| 6 | Highlight window (flash frame) | KWin `highlightwindow/` | Not found | Yes | S | none | reject: draws an outline. The outline look is protected by project rules. |
| 7 | Dim inactive windows | KWin `diminactive/` | Not found | Yes | S | none | reject: changes window appearance. |
| 8 | Magnifier and zoom | KWin `magnifier/`, `zoom/` | 6 zoom hits, unrelated | Yes | L | per-frame capture | reject: needs continuous screen capture, which costs idle CPU and memory. |
| 9 | Activities | `kwin/src/activities.cpp`, `plasma-desktop/applets/showActivityManager/` | Not found. `taskbarActivities` is a taskbar option (`crates/flamewm-platform/src/settings.rs:323`) | Yes | L | per-activity state | reject: conflicts with the fixed two-row grid in `docs/ARCHITECTURE.md`. |
| 10 | Minimize all | KWin `minimizeall/` | Show desktop present (`crates/flamewm-platform/src/shortcuts.rs:7-8`) | No | n/a | none | covered. Duplicate. |
| 11 | Overview and present windows | KWin `overview/`, `windowview/` | Overview hot corner (`crates/flamewm-platform/src/settings.rs:115,122`) | Partial | n/a | n/a | covered. Layout not compared; visual. |
| 12 | Quick tile and edge snap | KWin `tileseditor/`, `screenedge/`, `placement.cpp` | Keyboard tile (`crates/flamewm-api/src/shortcuts.rs:47-51`), `drag_edge.rs`, `keyboard_tile.rs`, edge snap (`settings.rs:461`) | No | n/a | n/a | covered. |
| 13 | Night Light | KWin `nightlight/` (`nightlightmanager.h`) | `crates/flamewm-settings-core/src/night_light.rs`, `gamma.rs` (Wayland) | No (Wayland) | n/a | n/a | covered. X11 path `unverified`. |
| 14 | KRunner | `plasma-workspace/krunner/` | `crates/flamewm-finder/src/state.rs:313,323` (`Item::Command`, `run_command`) | No | n/a | n/a | covered. Plugin breadth not compared. |
| 15 | Notification DND | `libnotificationmanager/kcfg/donotdisturbsettings.kcfg` | `notification_history.rs:41,85` (`SuppressReason::DoNotDisturb`) | No | n/a | n/a | covered. DND settings toggle is in flight, excluded. |
| 16 | Accessibility panel (KCM access) | `plasma-desktop/kcms/access/`, KWin `bouncekeys/`, `slowkeys/`, `dwellclicker/`, `mousekeys/` | None of these in `crates/` | Yes (group) | M | none | defer: dwell click and mouse keys need timers and grab changes. Sticky keys is rank 2. |
| 17 | Breeze theme | `breeze` repo (not cloned) | n/a | unverified | n/a | n/a | defer until the theme is cloned and viewed. |
| 18 | Settings search | `plasma-desktop/kcms/` | Not found (`unverified`, not searched exhaustively) | Unverified | M | none | defer. |
| 19 | Desktop effects stack (fade, glide, wobbly, magic lamp) | KWin `fade/`, `glide/`, `wobblywindows/`, `magiclamp/` | none | Yes, visual | L | per-frame GPU | reject: visual and idle cost. |
| 20 | Blur and translucency | KWin `blur/`, `translucency/` | excluded by project rules | n/a | n/a | n/a | reject. |

## Rejected and why (summary)

- Highlight window, dim inactive, magnifier, wobbly/magic-lamp effects: visual or continuous-capture cost, or prohibited by the look rules.
- Activities: conflicts with the fixed two-row workspace grid.
- Blur: prohibited.
- Minimize all: duplicate of show desktop.
