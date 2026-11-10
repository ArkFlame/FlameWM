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
- `bt-reply`: `BluetoothDeviceControl::on_ready` drains the facade connection through a reactor source (`LinuxIntegrationSource::Bluetooth`, registered in `flamewm-wm-x11`). Success replies are consumed silently; each error reply is logged once. Pure classifier test (`replies_are_consumed_and_only_error_replies_are_logged`); no live bus.
- `bt-plumb`: `flamewm_api::system::{BluetoothDeviceOp, BluetoothDeviceAction}` (address validated by `BluetoothDeviceAction::new`), `SystemAction::BluetoothDevice`, control method `BluetoothDevice(op, address, expectedRevision)` with op `pair|trust|untrust|remove` (one method, not three), revision-fence bypass as for power profiles, routed to `request(op, hci0, address)`. Errors surface through the control reply. Mock tests: control-wire round trip and malformed op/address rejection; platform bypass-and-handler test.

## Open rows (ordered)

1. `bt-agent`: `org.bluez.Agent1` registration for PIN/passkey prompts. Required before Pair succeeds for most devices. Security-sensitive.
2. `bt-list`: device list and adapter `Powered` state (`GetManagedObjects`, `PropertiesChanged`, no polling).
3. `bt-ui`: quick-controls row (plain, in the existing list). No visuals in scope yet.

## Unverified

- `bt-reply` and `bt-plumb` are verified by pure and mock tests only; no BlueZ round-trip, no real Pair/Trust/Remove.

- No real BlueZ, adapter or device was used. All checks are pure message-shape tests.
- No runtime D-Bus round-trip; `bluetoothd` behaviour on Pair/Trust/Remove is from the BlueZ API docs, not run.
