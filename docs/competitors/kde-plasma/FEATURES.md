# KDE Plasma: feature inventory and comparison with FlameWM

Snapshot: 2026-11-08. Competitor: KDE Plasma (KWin compositor, plasma-desktop, plasma-workspace), GPL-2.0 or later / LGPL per each repo's `LICENSES/`.
Source: shallow clones of `invent.kde.org/plasma/{kwin,plasma-desktop,plasma-workspace}.git` at `0406762`, `cc274e2`, `b0f268a`, held outside the repo. KDE is not in `.vendor/`.

## Method and limits

- Feature claims are mapped to KDE owner directories by name (`kwin/src/plugins/<effect>/`, `plasma-workspace/klipper/`, `plasma-workspace/krunner/`, `plasma-workspace/libnotificationmanager/`, `plasma-desktop/kcms/`). Config defaults were read from `.kcfg` files.
- FlameWM status is from `crates/**/*.rs` keyword probes (`grep -ril`, `.rs` only) and direct reads of the cited lines. Keyword hits prove code exists, not that it works. Those rows say `partial` or `unverified`.
- No KDE build or run was performed. No KDE memory or CPU number is measured here: every footprint cell is `unmeasured`.
- Breeze theme (`breeze` repo) was not cloned. Breeze rows are `unverified`.
- Verdict column is for the ideas in `LEARNINGS.md`. Items marked `covered` are not gaps.

## 1. Window management and compositing (KWin)

| Feature | KDE detail | KDE source (under `kwin/src/plugins/`) | FlameWM status | Evidence |
| --- | --- | --- | --- | --- |
| Overview (all windows, grid) | Effect shows all windows and desktops | `overview/` | covered (hot corner action) | `crates/flamewm-platform/src/settings.rs:115,122` (`HotCornerAction::Overview`). Layout and animation not compared. |
| Present windows | Effect scales windows of one desktop | `windowview/` | partial | Overview hot corner above. No per-desktop present layout found. |
| Quick tiling and tile editor | Half/quarter tiles, custom tile layouts | `tileseditor/`, `src/placement.cpp` | covered (keyboard and edge) | `crates/flamewm-api/src/shortcuts.rs:47-51` (`ACTION_TILE_*`), `crates/flamewm-wm-x11/src/wm/keyboard_tile.rs`, `crates/flamewm-wm-x11/src/wm/drag_edge.rs`. No custom tile editor. |
| Edge snapping and screen edges | Screen-edge actions and hot corners | `screenedge/` | covered | `crates/flamewm-platform/src/settings.rs:461-463,620-621` (`edge_snap_enabled`, `edge_snap_threshold_px: 16`). |
| Show desktop / minimize all | Minimize every window | `minimizeall/` | covered (show desktop) | `crates/flamewm-platform/src/shortcuts.rs:7-8` (`ACTION_SHOW_DESKTOP`, `_CTRL_ALT`). No separate minimize-all. |
| Workspace switch OSD | Transient on-screen name or number on switch | `desktopchangeosd/` | gap | 0 `.rs` hits for workspace OSD. Other OSDs exist (`crates/flamewm-assistant/src/osd.rs`, backlight and audio) but not for workspaces. |
| Highlight window | Flash frame to locate a window | `highlightwindow/` | gap | 0 `.rs` hits. Rejected: outline look is protected (see `LEARNINGS.md`). |
| Shake cursor to locate | Enlarge pointer after a shake gesture | `shakecursor/` | gap | 0 `.rs` hits for shake gestures (`shake` hits are unrelated). |
| Dim inactive windows | Lower opacity or brightness of unfocused windows | `diminactive/` | gap | 0 `.rs` hits. Rejected: changes the window look. |
| Magnifier and zoom | Screen magnification | `magnifier/`, `zoom/` | gap | 0 `.rs` hits for magnifier. Rejected: per-frame capture cost. |
| Blur and translucency | Compositor blur | `blur/`, `translucency/` | excluded | FlameWM forbids blur and surface changes (`AGENTS.md`, project rules). |
| Window animations | Fade, glide, magic lamp, wobbly, slide | `fade/`, `glide/`, `magiclamp/`, `wobblywindows/`, `slide/` | not compared | Visual. Not evaluated as gaps. |
| Activities | Per-activity desktops and window sets | `kwin/src/activities.cpp` (74 files mention `Activit`) | gap, rejected | `taskbarActivities` (`crates/flamewm-platform/src/settings.rs:323`) is a taskbar option, not an activity model. Fixed two-row grid per `docs/ARCHITECTURE.md`. |
| Compositor and GPU | Qt Quick effects, OpenGL/Vulkan scene | `kwin/src/scene/`, `compositor.cpp` | not compared | FlameWM has its own render path (`crates/flamewm-render-x11`, `crates/flamewm-wm-wayland`). |

## 2. Shell, launcher and system services (plasma-workspace, plasma-desktop)

| Feature | KDE detail | KDE source | FlameWM status | Evidence |
| --- | --- | --- | --- | --- |
| KRunner | Single-line search and command runner | `plasma-workspace/krunner/` (`view.cpp`, `qml/`) | covered | `crates/flamewm-finder/src/state.rs:313,323` (`Item::Command`, `run_command`). Plugin set not compared. |
| Night Light | Warm colour temperature on a schedule | `kwin/src/plugins/nightlight/` (`nightlightmanager.h`) | covered | `crates/flamewm-settings-core/src/night_light.rs`, `crates/flamewm-wm-wayland/src/protocols/gamma.rs:23,53` (Wayland gamma). X11 path `unverified`. |
| Klipper clipboard history | History of text, images and files; size and actions | `plasma-workspace/klipper/` (`historymodel.cpp`, `klipper.kcfg`) | gap | `klipper.kcfg:54-58`: `MaxClipItems` default `20`, max `2048`. `PreventEmptyClipboard` at `:17`. FlameWM keeps only the last text: `crates/flamewm-clipboard/src/keeper.rs:21` (`MAX_KEPT_BYTES = 1 << 20`). No history. |
| Password-manager exclusion | Skip history for password owners | `klipper/historymodel.cpp` (`x-kde-passwordManagerHint`) | covered in keeper | `crates/flamewm-clipboard/src/keeper.rs:24` (`PASSWORD_HINT`), never kept. |
| Notifications and DND | Pop-up suppression and history | `plasma-workspace/libnotificationmanager/` (`fullscreentracker.cpp`, `kcfg/donotdisturbsettings.kcfg`) | covered | `crates/flamewm-integrations-linux/src/notification_history.rs:41,85` (`SuppressReason::DoNotDisturb`). DND toggle is in flight, excluded. |
| Notification history bounds | Capped history | `libnotificationmanager/limitedrowcountproxymodel.cpp` | in flight, excluded | |
| Activities service | Activity manager daemon | `plasma-workspace/shell/`, `plasma-desktop/applets/showActivityManager` | gap, rejected | See Activities above. |
| Tray (StatusNotifier) | Tray protocol | `plasma-workspace/applets/systemtray/` | covered | 74 `.rs` hits for tray or StatusNotifier across `crates/` (`flamewm-tray`). |
| Screenshot | Region, window, screen | `kwin/src/plugins/screenshot/` | covered | `crates/flamewm-screenshot`. |
| Lock screen | Session lock | `kscreenlocker` (not cloned) | covered | `crates/flamewm-lock`. |
| Accent colour | Accent from wallpaper or theme | `plasma-workspace/kcms/colors/` | in flight, excluded | |
| Icon theme and taskbar alignment | Theme-driven icons | Breeze and Plasma icons | in flight, excluded | |

## 3. Settings (System Settings, plasma-desktop/kcms)

Directory list from `plasma-desktop/kcms/`: `access`, `activities`, `baloo`, `dateandtime`, `desktoppaths`, `keyboard`, `keys`, `mouse`, `touchpad`, `touchscreen`, `tablet`, `workspaceoptions`, `runners`, `spellchecking`, `ksplash`, `ksmserver`, `recentFiles`, `landingpage`, `gamecontroller`, `libkwindevices`.

| Area | KDE detail | FlameWM status | Evidence |
| --- | --- | --- | --- |
| Settings categories | Search and grouped pages | partial | `crates/flamewm-settings/src/` has `pager.rs`, `nav.rs`, `hotkeys.rs`, `startup_page.rs`, `theme_picker.rs`, `file_manager.rs`. No settings search found (`unverified`). |
| Accessibility | `access/`, `keys/` (sticky, slow, bounce keys) | gap | 0 `.rs` hits for sticky or slow keys. Earlier `sticky` hits were unrelated (`geometry_memory.rs`, `menus.rs`). |
| Input | Mouse, keyboard, touchpad pages | partial | `crates/flamewm-settings/src/input.rs`, `app_impl_2.rs` (pointer and accel keywords). |

## 4. Theme and visuals

| Area | KDE detail | FlameWM status |
| --- | --- | --- |
| Breeze | Default Qt and KDE theme | `unverified`: not cloned. Breeze screenshots not captured. |
| Light and dark | Global theme switch | covered (`crates/flamewm-settings/src/theme_picker.rs`) |

## 5. Strengths, weaknesses and memory

| Dimension | KDE | FlameWM | Note |
| --- | --- | --- | --- |
| Feature breadth | Very high: effects, activities, KRunner, Klipper, KCMs | Narrower, typed core | KDE's breadth is the reason its footprint is large. |
| Footprint | `unmeasured` | `unmeasured` (this pass) | No measurement. Do not compare numbers. |
| Config model | KConfig and kcfg per plugin, with `kconf_update` migrations | Typed settings (`flamewm-settings-core`, `flamewm-platform/src/settings.rs`) | The typed model is kept. |
| Licence | GPL-2.0 or later, LGPL (per repo `LICENSES/`) | FlameWM project licence | Ideas only. No code is copied. |

## Gaps for FlameWM (verified absent)

1. Clipboard history (Klipper): gap, see `WINNERS.md` rank 1.
2. Sticky and slow keys (KDE `access`/`keys`): gap, see rank 2.
3. Shake-to-locate cursor (`shakecursor`): gap, see rank 3.
4. Workspace switch OSD (`desktopchangeosd`): gap, see rank 4 (conditional).
5. Rejected gaps: highlight window, dim inactive, magnifier, activities.
