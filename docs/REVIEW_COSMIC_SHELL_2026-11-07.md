# COSMIC vs FlameWM shell review, 2026-11-07

Scope: read-only review of `.vendor/cosmic-epoch` against `crates/flamewm-shell`, `flamewm-desktop`, `flamewm-integrations-linux`, `flamewm-integrations-core`, `flamewm-platform`, `flamewm-settings-core`, `flamewm-tray`, `flamewm-session-core`, `flamewm-wm-wayland`. Method: grep plus bounded excerpts. No runtime measurement was run, so every win below is a hypothesis to measure.

Paths are repo-relative. `COSMIC` paths are under `.vendor/cosmic-epoch/`.

## Top 12 (ranked)

| # | Technique | COSMIC evidence | FlameWM today | Win | Effort | Risk |
|---|---|---|---|---|---|---|
| 1 | Notification actions and `ActionInvoked` | `cosmic-notifications/src/subscriptions/notifications.rs:296-310` advertises `action-icons`, `body-markup`, `sound` caps | `crates/flamewm-integrations-linux/src/notifications.rs:405` binds actions as `_actions` and discards them; `:362` advertises only `body`; no `ActionInvoked` emitter found | App support (buttons in Chrome, Slack, Mail toasts) | M | Med: needs a signal path and invoke routing back to the client |
| 2 | Image hints (`image-data`, `image_data`, `icon_data`, `image-path`) and `desktop-entry` icon fallback | `cosmic-notifications-util/src/lib.rs:48-69` parses `desktop-entry`, `image-data` variants | `notifications.rs:528-546` `parse_hints` reads only `urgency` and `value`; grep finds no `image-data` in `crates/` | Real avatars and app icons in toasts; no-icon toasts fixed | M | Med: untrusted pixel data, must cap dimensions and bytes before decode |
| 3 | Urgency-aware expiry and `transient` skip-history | `cosmic-notifications/src/app.rs:283-291` (per-urgency max timeout); `subscriptions/notifications.rs:380` `if !n.transient()` | `notifications.rs:556-566` `expiration()` is timeout-only; `notification_history.rs` has `popup_decision(app, urgency)` but no `transient` | Correct expiry for critical vs low; transient popups stay out of history | S | Low |
| 4 | Wallpaper: slideshow timer, source-dir inotify, bounded decode | `cosmic-bg/src/wallpaper.rs:320-351` rotation via calloop `Timer`; `:291-303` `notify` watch; `:369-380` `limits` on decode; `img_source.rs` | `crates/flamewm-desktop/src/wallpaper_loader.rs:164` worker thread with condvar; grep for `slideshow`/`rotation` in `flamewm-desktop`, `flamewm-settings-core`, `flamewm-image-core` returns nothing | Feature parity; no full-res retention per output | M | Med: rotation must not re-decode the whole folder each tick |
| 5 | Session: each child in a systemd scope, plus env import for activation | `cosmic-session/src/main.rs:541-553` `spawn_scope` per process; `:25` `get_systemd_env`; `:140-142` `ExponentialBackoff`; `:259-261` readiness ordering | `crates/flamewm-wm/src/bin/flamewm-session.rs:688` uses `ProcessSupervisorPolicy` (good, has backoff in `crates/flamewm-session-core/src/lib.rs:557-569`). Grep finds no `systemd-run`, `UpdateActivationEnvironment`, or `import-environment` in `crates/` | Cgroup cleanup of orphan children; D-Bus-activated apps see `WAYLAND_DISPLAY`/`DISPLAY` | M | Med: depends on systemd user session; needs a non-systemd fallback |
| 6 | Crash isolation for tray and heavy shell features | `cosmic-applets/cosmic-applet-status-area/src/subscriptions/status_notifier_watcher/` runs the watcher inside the applet process; `cosmic-panel` is a separate process | Helpers are isolated (`crates/flamewm-shell/src/quick_controls/supervisor.rs:92,297`, bounded restart at `:15`, `:297`; weather via `curl` child `weather_provider.rs:216`). Tray watcher is in-process (`crates/flamewm-tray/src/watcher.rs:75-88`) | A tray or D-Bus-menu crash does not take the shell down | L | High: crosses the "one owner per mechanism" rule in `docs/ARCHITECTURE.md`; needs a `docs/ARCHITECTURE_EXCEPTIONS.toml` entry if it moves |
| 7 | Workspace overview thumbnails via capture filter | `cosmic-workspaces-epoch/src/main.rs:372-400` `update_capture_filter` captures only visible workspaces and toplevels; `backend/wayland/toplevel.rs:25` adds capture source on demand | `crates/flamewm-shell-core/src/overview.rs:2,13` only divides the area into cells; grep for `screencopy`, `ext_image_copy`, `image_capture` in `crates/` finds nothing | Live overview thumbnails at low CPU, since only on-screen sources update | L | High: new Wayland protocol path in `flamewm-wm-wayland` |
| 8 | Idle dim, lock and suspend via idle-notify | `cosmic-idle/` (4 `.rs` files) and `cosmic-settings-daemon` `IdleManager` path | Idle inhibit exists (`crates/flamewm-wm-wayland/src/protocols/compat.rs:125` `IdleInhibitHandler`). No idle timeout or `ext-idle-notify` found (grep `idle_notify`, `idle_timeout`, `suspend_after` empty) | Automatic dim/lock/suspend; power savings | M | Med: must respect inhibitors already handled at `compat.rs:36-82` |
| 9 | Config and external-edit watching | `cosmic-panel/cosmic-panel-bin/src/config_watching.rs:100` `watch_config`, `watch_cosmic_theme` | Settings change via control-dbus signals (`crates/flamewm-desktop/src/settings_sync.rs`), so hand-edited settings files are ignored. No `inotify`/`notify` in `flamewm-platform` or `flamewm-settings-core` | Edits from a text editor or dotfile sync apply live | S | Low; must debounce writes from our own `SettingsWriter` (`crates/flamewm-platform/src/services/settings_persist.rs:50-67`) to avoid echo loops |
| 10 | Brightness: coalesce writes on a worker thread | `cosmic-settings-daemon/src/brightness_device.rs:41-106` DDC thread with condvar, latest-value-wins, `IDLE_TIMEOUT` | `crates/flamewm-wm-wayland/src/control/media.rs:28-41` steps backlight per key press (not verified whether the write blocks the event loop) | Key-repeat cannot stall the compositor or shell | S | Low; needs a read of `media.rs` to confirm the write is on the hot path |
| 11 | Launcher: persisted recent-use ranking | `pop-launcher/service/src/lib.rs:85-108` `store_cache`/`load_cache` for `recent`; `:342-368` `cache_identifier` boosts recents | Fuzzy match exists (`crates/flamewm-finder/src/search.rs:11`), not verified whether recents persist. Recent-use grep in `flamewm-applications` was not run | Better first-result ranking without extra CPU at query time | S | Low |
| 12 | Notification body markup: advertise and sanitize | `cosmic-notifications-util/src/markup.rs:23` parses spec markup into spans (`app.rs:25` `html_to_spans`) | `notifications.rs:362` advertises only `body`, so clients strip markup themselves. Our shell renders the raw string | Consistent rendering of `<b>`, `<a>`, and entities | S | Low; must never render unsanitized markup as HTML |

## Already present in FlameWM (keep)

- Replace IDs and timeouts: `notifications.rs:425-439`, `:556-566`.
- Progress hint: `notifications.rs:528-546` (`value` 0-100).
- Signal-driven steady state, no polling loop: `crates/flamewm-shell/src/app/timers.rs:148` ("Steady state never polls"). COSMIC polls nothing comparable in the reviewed paths.
- Child supervisor with bounded restart and reap: `crates/flamewm-shell/src/quick_controls/supervisor.rs:15,297`; `crates/flamewm-session-core/src/lib.rs:557-569`.
- Backoff on reconnect: `crates/flamewm-integrations-core/src/network.rs:90-96`, `pulse.rs:43-49`, `backoff.rs`.
- Night Shift gamma: `crates/flamewm-wm-wayland/src/protocols/gamma.rs:1`.
- Brightness keys and OSD: `crates/flamewm-wm-wayland/src/control/media.rs:28`; `crates/flamewm-shell/src/osd.rs:278` (lazy, no idle allocation).
- Tray watcher: `crates/flamewm-tray/src/watcher.rs` (`org.kde.StatusNotifierWatcher`).
- Device and removable watches driven by inotify/reactor: `crates/flamewm-integrations-linux/src/device_watch.rs:56`, `crates/flamewm-desktop/src/downloads.rs:18`.

## Not recommended

- Migrating from the `dbus` crate reactor to `zbus` async. COSMIC uses zbus (`cosmic-applet-status-area/.../server.rs:9`), but our reactor (`crates/flamewm-dbus-reactor/src/lib.rs:33-171`) already drives the bus without polling. Effort L, win unproven.
- Inline reply. Grep for `inline-reply` finds nothing in COSMIC either, so it is not a COSMIC technique.

## Next actions

1. Item 3 (S): add `transient` and urgency timeouts in `notifications.rs:556`, with a unit test next to `:615`.
2. Item 9 (S): add a debounced config-file watch in `flamewm-platform` that ignores writes from `SettingsWriter`.
3. Item 1 (M): emit `ActionInvoked` and keep actions in `notifications.rs:405`; advertise `actions` in `:362`.

Skipped or not checked: runtime CPU/RAM measurement; `cosmic-applets` audio, network and bluetooth applets; `cosmic-osd` internals; `cosmic-launcher` UI; `flamewm-settings` UI code; `flamewm-wm-wayland/src/control/media.rs` blocking behavior (item 10 unverified). Risk: items 6, 7 cross the architecture boundary and need `docs/ARCHITECTURE.md` review first.
