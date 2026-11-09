# Competitor feature gap table

Snapshot: origin/main 32f28041, 2026-11-09. Competitor source is read-only from `.vendor/vela-main` (Vela). Status values: `absent` (no hits), `partial`, `present`, `unverified` (no runtime proof). Rank = user-visible gap x fit with the current architecture.

| Rank | Gap | Competitor evidence (Vela) | FlameWM evidence (origin/main) | Status | Fit / next step |
| --- | --- | --- | --- | --- | --- |
| 1 | Power profile switch (UPower PowerProfiles) | `.vendor/vela-main/shell/src/systemstatus.cpp:54,59`; `settings/qml/pages/PowerPage.qml` | 0 hits for `power-profiles`, `PowerProfiles`, `power_profile` in `crates/**/*.rs` | absent | Small D-Bus client in `flamewm-integrations-linux` (reactor-owned bus), settings toggle, quick-controls tile. Multi-crate + UI, needs runtime check on a host with power-profiles-daemon. |
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
