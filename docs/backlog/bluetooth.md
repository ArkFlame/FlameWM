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

- `bt-list`: `BluetoothDeviceControl::snapshot()` returns a typed `BluetoothSnapshot { powered, devices: Vec<Device{path,name,paired,trusted,connected}> }` for adapter `hci0`. Source: one `org.freedesktop.DBus.ObjectManager.GetManagedObjects` call sent in `connect`, its reply consumed in `on_ready` by serial (`pending`), then kept current by `PropertiesChanged` (`org.freedesktop.DBus.Properties`), `InterfacesAdded` and `InterfacesRemoved`. Two `AddMatch` rules (sender `org.bluez`) are sent before the list request. No polling, no new event source, no new dependency. Pure parser: `parse_managed_objects(&Message, adapter)`; pure state machine: `absorb`/`apply_signal`. Name is `Alias`, falling back to `Name`. Devices are kept only under the adapter path; GATT child objects are ignored. Pure tests: `managed_objects_reply_yields_powered_and_scoped_devices`, `properties_changed_updates_device_flags_and_adapter_power`, `interfaces_added_and_removed_track_devices`, `only_the_pending_serial_replaces_the_list`, `error_reply_to_pending_list_clears_pending_without_state_change`, `malformed_managed_objects_body_is_rejected`, `list_requests_are_pure_method_calls`. A failed list request is logged as an error reply, and the list stays empty.

- `bt-ui`: `SystemSnapshot.bluetooth` (`BluetoothSnapshot { available, powered, devices }`, device `path`/`address`/`name`/`paired`/`trusted`/`connected`) is published by the WM and encoded on the control wire as tab/newline records. The network popup shows a Bluetooth row (`#network-bluetooth-row`, existing `network-row` class): hidden when no adapter exists, else `Off`, `Connected: ...`, `Paired: ...` or `On, no paired devices`. Click (`bluetooth.trust.toggle`) sends the typed `SystemAction::BluetoothDevice` trust toggle for the primary device (first connected, else first paired). Connect is not in the typed action set, so the row does not connect. Pure tests: `projection/network.rs` `bluetooth_row_tests`, control-wire `bluetooth_snapshot_round_trips_and_absent_keys_decode_to_default`.

## Open rows (ordered)

1. `bt-agent`: `org.bluez.Agent1` registration for PIN/passkey prompts. Required before Pair succeeds for most devices. Security-sensitive.

## Unverified

- `bt-ui`: no real BlueZ and no device; the row is verified by pure projection and wire tests only. The popup layout (slot list trimmed from 200px to 140px to fit the row in the fixed 453px popup) is not checked on screen.
- `bt-list`: no real BlueZ; the `GetManagedObjects` reply and signal shapes are constructed in tests from the BlueZ API docs, not captured from `bluetoothd`. The `AddMatch` rules are not checked against a live bus. A `bluetoothd` restart is not re-listed (no `NameOwnerChanged` handling); the list stays as last seen until the next signal. On a machine without BlueZ, the list request logs one error reply at startup.
- `bt-reply` and `bt-plumb` are verified by pure and mock tests only; no BlueZ round-trip, no real Pair/Trust/Remove.

- No real BlueZ, adapter or device was used. All checks are pure message-shape tests.
- No runtime D-Bus round-trip; `bluetoothd` behaviour on Pair/Trust/Remove is from the BlueZ API docs, not run.

## Wayland parity (`bt-wayland`)

- Snapshot push: `crates/flamewm-wm-wayland/src/control/media.rs` `drain_bluetooth` (on each bus-fd wake and once at install) calls `on_ready`, then `update_bluetooth`, same order as X11 `crates/flamewm-wm-x11/src/runtime.rs:476`.
- Reply drain: `crates/flamewm-wm-wayland/src/control/mod.rs` `install` inserts a level source on `BluetoothDeviceControl::watch().fd`.
- Action: `crates/flamewm-wm-wayland/src/control/service.rs` routes `SystemAction::BluetoothDevice` to the shared seam `BluetoothDeviceControl::perform_action` (`crates/flamewm-integrations-linux/src/bluetooth.rs`), which X11 `crates/flamewm-integrations-linux/src/lib.rs` also calls.
- Test: `shared_device_action_seam_fails_soft_without_bus` (no bus, `Unavailable`).
- Unverified: no BlueZ adapter round-trip. Nested Wayland runtime evidence is recorded below.

## Runtime evidence (`ee29fae3`, nested Wayland, 2026-11-10)

Build: `cargo build --profile fast -p flamewm-wm -p flamewm-desktop -p flamewm-shell -p flamewm-wm-wayland` exit 0 (3m36s). Each run is a private Xvfb, `flamewm-wayland` winit backend with autostart of `flamewm-desktop` and `flamewm-shell`, 14 s to settle, then 10 s idle. Idle CPU is `/proc/<pid>/stat` utime+stime over 10 s at `CLK_TCK=100`.

Host: `bluetooth.service` is active, so BlueZ is present on this host. The system bus socket exists.

| Case | Setup | Runs | Idle CPU % | Panic/abort | Network popup | Bluetooth row |
| --- | --- | --- | --- | --- | --- | --- |
| a | host system bus, BlueZ active | 8 | 0.00, 0.40, 0.00, 0.10, 0.30, 0.00, 0.00, 0.00 | 0 | opens (screenshot checked in a-5) | shown: `Paired: BT5.0 KB` (real adapter) |
| b | `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent` | 3 | 0.00, 0.30, 0.00 | 0 | not opened; the Wi-Fi tray icon is absent, so the click hit Assistant | not visible (popup not opened) |
| c | system bus socket that accepts and closes (`python3 -I`) | 3 | 0.00, 0.00, 0.00 | 0 | not checked (same tray state as b expected) | not checked |
| d | real `dbus-daemon` on the system address, killed at t=14 s (connection drop while the compositor is live) | 3 | 0.00, 0.40, 0.00 | 0 | not checked | not checked |

- Log lines in case d: `NetworkManager dispatch failed: Unavailable: D-Bus connection read/write failed` and `bluetooth dispatch failed: Unavailable: D-Bus connection read/write failed`, one each, then no further lines in 10 s idle. This is the drop path for the level-triggered bus fd. It does not spin.
- Cases b and c logs have no `org.bluez` or NetworkManager error lines (the bus is dormant or closed before the handshake, so the drain has no pump to fail).
- Result: no spin, no panic, no code change. The risk named for `ee29fae3` (level-triggered fd on a dead connection) is not reproduced.
- Unverified: the Bluetooth row was not checked in the hidden state on screen (b, c, d popups did not open). Connect is not exercised. Only a-5 has a screenshot inspected; a-6..8 and b/c/d idle screenshots were not visually checked.
