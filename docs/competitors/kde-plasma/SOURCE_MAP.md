# KDE Plasma source map

Competitor: KDE Plasma. Read-only reference in a scratch clone, not in `.vendor/` and not in this repo.
Clones: `kwin` at `0406762`, `plasma-desktop` at `cc274e2`, `plasma-workspace` at `b0f268a` (all `--depth 1`, fetched 2026-11-08 from `invent.kde.org/plasma/`).
Licence: each repo ships `LICENSES/` (GPL-2.0/3.0, LGPL, MIT, BSD). Ideas only; no code is copied.

Evidence rule: paths were checked with `test -e`. Counts come from `grep -ril`. Config defaults are read from `.kcfg`. No KDE file was built or run.

## 1. Repos and owners

| Area | Repo and path | Notes |
| --- | --- | --- |
| Compositor and WM | `kwin/src/` (`compositor.cpp`, `placement.cpp`, `activities.cpp`, `workspace.cpp`, `scene/`) | Effects are plugins under `kwin/src/plugins/<name>/` (about 100 directories). |
| Effects (examples) | `kwin/src/plugins/{overview,windowview,tileseditor,screenedge,nightlight,desktopchangeosd,highlightwindow,shakecursor,diminactive,stickykeys,magnifier,minimizeall,screenshot}/` | Presence checked by `ls`. |
| Accessibility (KWin) | `kwin/src/plugins/{stickykeys,bouncekeys,slowkeys,dwellclicker,mousekeys}/` | |
| Clipboard | `plasma-workspace/klipper/` (`historymodel.cpp`, `klipper.kcfg`, `klipper.cpp`) | `MaxClipItems` at `klipper.kcfg:54`. `PreventEmptyClipboard` at `:17`. Password hint in `historymodel.cpp`. |
| Launcher | `plasma-workspace/krunner/` (`view.cpp`, `main.cpp`, `qml/`, `dbus/`) | Runner plugins are not in this clone. |
| Notifications | `plasma-workspace/libnotificationmanager/` (`notifications.cpp`, `fullscreentracker.cpp`, `kcfg/donotdisturbsettings.kcfg`) | DND config in `kcfg/`. |
| Tray | `plasma-workspace/applets/systemtray/` | StatusNotifier host. |
| Activities | `plasma-desktop/applets/showActivityManager/`, `kwin/src/activities.cpp` | 74 `kwin/src` files mention `Activit`. |
| Settings (KCMs) | `plasma-desktop/kcms/` (`access`, `keys`, `keyboard`, `mouse`, `workspaceoptions`, `runners`, `activities`, `touchpad`, ...) | Colour KCM is in `plasma-workspace/kcms/colors/`. |
| Night light | `kwin/src/plugins/nightlight/` (`nightlightmanager.h`, `nightlightsettings.kcfg`) | Also `kwin/src/backends/drm/drm_output.cpp`. |

## 2. Build and tooling

| Area | Fact | Anchor |
| --- | --- | --- |
| Build | CMake, Qt 6, KF6 | `CMakeLists.txt` in each repo |
| Config | KConfig `.kcfg` per plugin, `kconf_update` migrations | `plasma-workspace/shell/kconf_update/`, `plasma-workspace/libnotificationmanager/kconf_update/` |
| Licence files | `kwin/LICENSES/`, `plasma-desktop/LICENSES/`, `plasma-workspace/LICENSES/` | per repo |

## 3. FlameWM owners for the same concerns

| Concern | FlameWM owner (path) |
| --- | --- |
| WM policy, tiling, snap | `crates/flamewm-wm-x11/src/wm/` (`keyboard_tile.rs`, `drag_edge.rs`) |
| Shortcuts | `crates/flamewm-api/src/shortcuts.rs`, `crates/flamewm-platform/src/shortcuts.rs` |
| Settings | `crates/flamewm-platform/src/settings.rs` (typed, 1345 lines), `crates/flamewm-settings-core/` |
| Clipboard | `crates/flamewm-clipboard/src/{keeper,owner,codec,lib}.rs` |
| Launcher | `crates/flamewm-finder/` |
| Notifications | `crates/flamewm-integrations-linux/src/notification_history.rs` |
| Night light | `crates/flamewm-settings-core/src/night_light.rs`, `crates/flamewm-wm-wayland/src/protocols/gamma.rs` |
| Input (Wayland) | `crates/flamewm-wm-wayland/src/input/{mod,shortcuts,buttons}.rs` |
| Settings UI | `crates/flamewm-settings/src/` |

## 4. Not audited

- KDE runner plugins, Breeze theme, and the KWin scene graph were not read line by line.
- FlameWM rows are keyword and path checks. Behaviour was not run.
