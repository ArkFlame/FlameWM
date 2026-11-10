# Competitor feature gap table

> Rows: `cmp-quick`, `cmp-power`, `cmp-dnd`, `cmp-bt`, `cmp-polkit`, `cmp-a11y`, `cmp-alttab`, `cmp-volume`, `cmp-wm-session-lock` in `docs/BACKLOG.md`.

Snapshot: origin/main 32f28041, 2026-11-09. Competitor source is read-only from `.vendor/vela-main` (Vela). Status values: `absent` (no hits), `partial`, `present`, `unverified` (no runtime proof). Rank = user-visible gap x fit with the current architecture.

| Rank | Gap | Competitor evidence (Vela) | FlameWM evidence (origin/main) | Status | Fit / next step |
| --- | --- | --- | --- | --- | --- |
| 1 | Power profile switch (UPower PowerProfiles) | `.vendor/vela-main/shell/src/systemstatus.cpp (line unpinned),59`; `settings/qml/pages/PowerPage.qml` | 0 hits for `power-profiles`, `PowerProfiles`, `power_profile` in `crates/**/*.rs` | absent | Small D-Bus client in `flamewm-integrations-linux` (reactor-owned bus), settings toggle, quick-controls tile. Multi-crate + UI, needs runtime check on a host with power-profiles-daemon. |
| 2 | Quick settings sheet in one place (Wi-Fi, BT, power, brightness) | `shell/qml/`, `brightness/`, `bluetooth/` (FEATURES.md 1.2 L209-214) | `crates/flamewm-shell/src/quick_controls/host/run.rs` exists; NetworkManager hits 18 | partial | Extend existing quick controls rather than a new surface. |
| 3 | Bluetooth pairing and in-shell prompts | `.vendor/vela-main/bluetooth/` (25 files) | 1 keyword hit under `crates/` | absent | Large: new daemon client, agent prompts. Separate plan. |
| 4 | Polkit authentication agent | `.vendor/vela-main/polkit/` (17 files) | 0 hits | absent | Medium-large; security-sensitive, needs its own owner crate. |
| 5 | Accessibility: magnifier, colour filters, sticky keys | `shell/src/accessibility.h`, `compositor/src/color.h` | 2 `magnif` hits, no implementation | absent | Large (render path). Not in this step. |
| 6 | Alt+Tab live previews | `shell/src/windowcapture.cpp` | 15 alt-tab hits, no preview capture code found | unverified | Render-heavy; conflicts with idle-memory policy in `.agents`/GAP plan. |
| 7 | Per-app volume (wheel over taskbar button) | `docs/audio.md`; Quick settings sound page (FEATURES.md 1.2) | `crates/flamewm-integrations-linux/src/pulse.rs:180` `set_sink_input_volume`, `:223` mute | partial | Backend exists; missing the taskbar wheel and per-app UI. Small once taskbar owner allows (dock.rs is off-limits this step). |
| 8 | Night light | `compositor/src/color.h`, `sun.h` (sunset schedule offline) | `crates/flamewm-settings-core/src/night_light.rs`, `crates/flamewm-wm-wayland/src/protocols/gamma.rs` | present (Wayland gamma), unverified at runtime | Covered; Vela row is out of date. |
| 9 | Do Not Disturb toggle | Vela `shell/` notifications with DND (FEATURES.md 1.2 L217-218) | Wired end-to-end: `crates/flamewm-integrations-linux/src/notifications.rs:178` `set_dnd` -> persisted via `persist_history`; caller `crates/flamewm-shell/src/main.rs:280-283` (`Command::ToggleDnd`) from `notification_center.rs:53,72,302`; UI button `ui/shell/notification-center.html:39`; tests `notifications/tests.rs:467`, `notification_history.rs:460,613` | present | No code change needed. `docs/competitors/vela/GAP_PLAN.md:77` says "no UI caller" and is stale. |
| 10 | Session lock on Wayland | Vela `lock/` (6 files), ext-session-lock | `crates/flamewm-wm-wayland/src/session_lock.rs`, `lock_global.rs` exist; README still lists "No session lock" | present in code, unverified at runtime | Doc drift; needs runtime proof before any README claim. |

## Step 2 result

- DND (rank 9) is already wired on origin/main. Nothing was added, so no duplicate toggle.
- The next open item by rank is #1 (power profiles). It was not implemented in this step: it touches the D-Bus integration, the settings facade, quick controls and possibly `ui/` goldens, which exceeds one small change. Needs its own plan.

## Unverified

- No runtime check of any row. Status is from source inspection only.
- Vela line numbers are from the local copy at `.vendor/vela-main` (snapshot 2026-11-08 per `docs/competitors/README.md`).

## Power-profile row and quick-controls audit (2026-11-10)

Code: `2b6f5cad` (power-profile row, battery popup), fix `db42c3b6` (quick-control host subscribes before initial load; rebased from `be2be796` onto `8f4676be`). Runtime: private Xvfb `:93`, `flamewm` + `flamewm-desktop` + `flamewm-shell` built from `db42c3b6` into `/run/media/linsaftw/SDD/gate-targets/ppv-run`. Host had `net.hadess.PowerProfiles` (power-profiles-daemon), profiles `power-saver, balanced, performance`.

Runtime evidence (screenshots viewed):

- Bug reproduced before the fix: fresh session showed the profile row as `--` while the service reported `ActiveProfile=performance`. The WM snapshot was correct (`com.arkflame.FlameWM1` `System.GetSnapshot` returned `powerProfilesAvailable=true`, 3 profiles). The quick-control host (`QuickControlSnapshot::load`, `run.rs`) loaded before `ControlSignalClient` subscribed, so a profile change in that window was never delivered. A later change to `balanced` updated the row to `Balanced`, which isolated the race.
- After the fix, a fresh session with the profile preset to `performance` opens the battery popup showing `Performance`, right-aligned on the `CPU usage` baseline. `CPU usage: 100%` is not truncated (`battery.html:18`, 160px).
- Click on the row (XTEST at the row) cycles `performance` to `power-saver` (wrap). `powerprofilesctl get` returned `power-saver`, and the popup re-projected `Power saver` with no clipping.
- No backend (`--`): the same default snapshot path produced `--` in the same slot, with no layout shift. Runtime for a host without any profile service was not reproduced (the service cannot be stopped without root). Covered by `battery_popup_tests.rs` (`--` for default snapshot; cycle returns `None`).
- No panic in `flamewm`, `flamewm-desktop`, `flamewm-shell` logs during the session.

Quick-controls audit (FlameWM `file:line` vs Vela `.vendor/vela-main/shell/qml/QuickSettings.qml`, read-only):

| Control | FlameWM (file:line) | Status | Vela reference |
| --- | --- | --- | --- |
| Volume lane per output | `ui/shell/audio.html:53` (`audio.slot.N.set`) | present | `QuickSettings.qml:774` (volume slider) |
| Mute per output | `ui/shell/audio.html:55` (`audio.slot.N.mute`) | present | `QuickSettings.qml:388` |
| Audio device/app tabs | `ui/shell/audio.html:37,39` | present | not in Vela quick panel |
| Wi-Fi toggle | `ui/shell/network.html:46` (`network.wifi.toggle`) | present | `QuickSettings.qml:689-693` (Wi-Fi tile) |
| Wi-Fi scan | `ui/shell/network.html:45` (`network.scan`) | present | `QuickSettings.qml:689-693` (networks page) |
| Wi-Fi network connect | `ui/shell/network.html:53-60` (`network.connect.N`) | present | `QuickSettings.qml` `networkRows` |
| Disconnect / connected row | `ui/shell/network.html:50` | present | `QuickSettings.qml:689-693` |
| Bluetooth trust toggle | `ui/shell/network.html:47` (`bluetooth.trust.toggle`) | partial (trust, not power) | `QuickSettings.qml:711-717` (`Bluetooth.setPowered`) |
| Bluetooth power on/off tile | none (no `bluetooth.power` action) | absent | `QuickSettings.qml:711-717` |
| Airplane mode | none in `ui/shell`, `crates/flamewm-shell/src` | absent | `QuickSettings.qml:721-725` |
| Night light | no quick tile; `crates/flamewm-api/src/blue_light.rs` exists (API only) | absent in quick controls | `QuickSettings.qml:735-740` |
| Energy saver / power profile | `ui/shell/battery.html:53` (`power.profile.cycle`, row not tile) | present (row, cycle) | `QuickSettings.qml:726-732` (`Status.setPowerSaver`) |
| Battery percent, state, rate | `ui/shell/battery.html:19-22` | present | `QuickSettings.qml:1089-1110`, `1311-1327` |
| CPU usage row | `ui/shell/battery.html:18` (160px) | present | not in Vela |
| Brightness | `ui/shell/battery.html` `battery-brightness-*` row (`brightness.down`/`brightness.up`, 10% steps, `SetBrightness`); OSD `ui/shell/osd.html:22` | present (row, step buttons; not a slider) | `QuickSettings.qml:486-512`, `761-765` |
| Accessibility page | none in `ui/shell/*.html` | absent | `QuickSettings.qml:747` |
| Calendar (tabs, prev/next/today, days) | `ui/shell/calendar.html` | present | not in Vela quick panel |
| Weather (location search, pick) | `ui/shell/weather.html:42-43,135-139` | present | not in Vela quick panel |

Status: runtime-verified for the power-profile row (display, alignment, cycle, service change). The audit is static (grep of `ui/shell`, `crates/flamewm-shell/src`), not runtime-verified per control.

## Unverified (power-profile pass)

- Host with no profile service: `--` shown by the same code path, but not run without a profile service.
- Failed `SetPowerProfile` (soft failure) not provoked at runtime.
- Battery-popup layout at other DPI or with long `Power saver`/`Performance` widths beyond the 100px row.
- The startup race fix has no deterministic regression test; verified by before/after runtime session only.

Rebased evidence (tree `db42c3b6` on `8f4676be`): gate `scripts/gate flamewm-shell flamewm-render-core flamewm-render-compiler` with `EXIT_GUARD=0 EXIT_FMT=0 EXIT_CLIPPY=0 EXIT_TEST=0 GATE_DONE all=0`. Runtime re-run on the rebased binaries: fresh session with profile `performance` shows `Performance` in the battery popup; click on the row set `power-saver`. Battery tray icon visibility is a shell-side snapshot, not part of this row; one fresh session showed no battery tray item (the shell's own power copy was not refreshed in time), and a second fresh session showed it. Not root-caused; unverified.
