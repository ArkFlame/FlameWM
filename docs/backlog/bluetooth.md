# Bluetooth scope (2026-11-10, origin/main 0de670a1)

> Rows: `cmp-bt` (partial, facade `b6bddef9`), `bt-reply`, `bt-plumb`, `bt-agent`, `bt-list`, `bt-ui` in `docs/BACKLOG.md`.

## What exists before this change

| Capability | Status | Evidence |
| --- | --- | --- |
| List devices / adapter state | absent | `grep -ri bluetooth crates/` hits only `crates/flamewm-shell/src/quick_controls/protocol.rs:391` (a test string) |
| Connect / disconnect / toggle power | absent | no `org.bluez` use in `crates/` |
| Pair / Trust / Remove | absent | no `org.bluez` use in `crates/` |
| Agent (PIN / passkey prompts) | absent | no `org.bluez.Agent1` |
| Quick-controls tile or row | absent | none |

Vela reference: `.vendor/vela-main/bluetooth/` (25 files), as in `docs/backlog/compare.md` rank 3.

## Landed in this slice

- `crates/flamewm-integrations-linux/src/bluetooth.rs`: `BluetoothDeviceControl` facade over the reactor-owned system `BusPump`. Typed `DeviceOp::{Pair, SetTrusted, Remove}` builders for `org.bluez.Device1` and `org.bluez.Adapter1.RemoveDevice`. Address and adapter names are validated before path construction.
- Pure builder tests mirror the `power_profiles.rs` pattern. No bus needed.

## Open rows (ordered)

1. `bt-reply`: drain and track BlueZ replies (serials, errors) through `on_ready`, wired by `flamewm-wm-x11` like `PowerProfilesProvider`. Until then replies queue unread on the facade connection.
2. `bt-plumb`: `flamewm-api` `BluetoothAction`, control-wire encoding, platform routing, and a `SetBluetoothDeviceTrusted`/`Remove`/`Pair` control action (mirror `4021f35f`).
3. `bt-agent`: `org.bluez.Agent1` registration for PIN/passkey prompts. Required before Pair succeeds for most devices. Security-sensitive.
4. `bt-list`: device list and adapter `Powered` state (`GetManagedObjects`, `PropertiesChanged`, no polling).
5. `bt-ui`: quick-controls row (plain, in the existing list). No visuals in scope yet.

## Unverified

- No real BlueZ, adapter or device was used. All checks are pure message-shape tests.
- No runtime D-Bus round-trip; `bluetoothd` behaviour on Pair/Trust/Remove is from the BlueZ API docs, not run.
