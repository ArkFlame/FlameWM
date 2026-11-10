# VPN / connection activation scope (2026-11-10, origin/main 88271686)

> Rows: proposed `net-vpn` (new), related to `cmp-quick` in `docs/BACKLOG.md`.

## What exists before this change

| Capability | Status | Evidence |
| --- | --- | --- |
| VPN activate / deactivate | absent | `grep -rli vpn crates/` = 0 files |
| Wi-Fi connect, known connect, disconnect, radio toggle | present | `crates/flamewm-integrations-linux/src/network_manager/actions.rs:50-120` (`SystemAction::{SetWifiEnabled, Disconnect, ConnectKnown, ConnectWifi}`) |
| Activate by profile path | present (Wi-Fi only) | `actions.rs` `queue_activate_connection` → `ActivateConnection(profile, device, ap)` |
| Deactivate by active-connection path | present (only `snapshot.active_path`) | `actions.rs:86-92` `DeactivateConnection` |

## Why the VPN slice was not landed

Target: `SystemAction::VpnConnection { action: Activate | Deactivate, id }` over the existing reactor-owned NetworkManager `BusPump`.

The Bluetooth pattern (`88271686`) fits only when the handler is a single method call with a known object path. VPN does not fit that shape:

1. `id` is a connection id or UUID, not an object path. `ActivateConnection` needs a settings path. Resolving it needs `org.freedesktop.NetworkManager.Settings.GetConnectionByUuid` (one reply hop, new `PendingRequest` variant, reply arm in `reply.rs`), or `ListConnections` + `GetSettings` fan-out to match `connection.id`.
2. `Deactivate` needs an active-connection object path. The provider only tracks Wi-Fi `active_path`. Resolving a VPN needs root `ActiveConnections` plus per-path `Connection.Id` or `Uuid` reads, a multi-reply state machine.
3. Both chains add reply state to `NetworkManagerProvider`. Estimated size with wire, platform and mock tests: over the 350-line cap for one slice.

## Proposed split (ordered)

1. `net-vpn-lookup`: settings-path resolution by UUID via `Settings.GetConnectionByUuid`. Pure message-shape test. Changes `mod.rs` (`PendingRequest`) and `reply.rs`.
2. `net-vpn-activate`: `SystemAction::VpnConnection { action: Activate, uuid }`. Validated UUID type in `flamewm-api`. Wire round trip, platform bypass-and-handler test, mock `ActivateConnection(path, "/", "/")` shape.
3. `net-vpn-deactivate` (landed): active-path lookup via root `ActiveConnections`, then `Connection.Active.Uuid` per path (sequential probe), then `DeactivateConnection(active_path)`. Pure builders and reply parsers in `vpn.rs`; error replies and an unknown UUID are logged or returned, not dropped. Verified by unit tests on message shape only.
4. `net-vpn-ui`: quick-controls row. Out of scope until 1-3 land.

## Unverified

- No real NetworkManager, no VPN profile, no D-Bus round trip. Nothing in this doc is runtime-proven.
- Message shapes come from the NetworkManager D-Bus API docs and the existing Wi-Fi code, not a live bus.
