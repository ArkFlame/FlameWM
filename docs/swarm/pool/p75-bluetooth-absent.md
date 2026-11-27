# Unit p75-bluetooth-absent
STATUS: DONE

Crate flamewm-integrations-linux bluetooth and shell bluetooth popup: no BlueZ on D-Bus, no adapter, rfkill blocked: tray item hidden or shows state, zero timers while absent. Tests for pure state mapping.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/bluetooth.rs:56-139 | No pure presence mapping existed; rfkill was never read, so a hard-blocked radio still looked like a normal adapter. | Added `BluetoothPresence`, `classify_bluetooth(service, adapter, rfkill, powered)`, `rfkill_is_blocked` and fail-soft sysfs `adapter_rfkill_blocked`; `timer_armed()` is false in every state. **UNBUILT** | rfkill read once at connect, not re-read: no polling ever (by design), so a post-start unblock needs a restart. Unreadable sysfs means "not blocked". |
| crates/flamewm-integrations-linux/src/bluetooth.rs:490-547 | The facade exposed raw adapter availability with no rfkill contribution. | Added `rfkill_blocked` field set from `adapter_rfkill_blocked(DEFAULT_ADAPTER)` in `connect()`; `snapshot()` forces `available = false` while blocked, hiding tray/popup. **UNBUILT** | Same cached-at-connect limitation; fail-soft default keeps existing behaviour on kernels without rfkill entries. |
| crates/flamewm-shell/src/projection/network.rs:842-871 | The popup row used its own ad-hoc `available`/`powered` guards, so tray and popup did not share one decision. | `bluetooth_row_status` now delegates to the pure `classify_bluetooth`: absent -> `None` (row hidden), Off -> `"Off"`, On -> unchanged device/status copy. **UNBUILT** | Passes `available` for both service and adapter presence and `false` for rfkill, since the integration already folded rfkill into `available`; no visual change when a device is present. |
| crates/flamewm-integrations-linux/src/bluetooth.rs:927,946 | Two test fixtures construct `BluetoothDeviceControl` literally and would miss the new field. | Added `rfkill_blocked: false` to both fixtures (tests, not production). **UNBUILT** | None beyond the usual unrun-test risk. |
| crates/flamewm-integrations-linux/src/bluetooth.rs:970-1015 | No DBus-free tests covered absence or timer arming. | Added four tests: no BlueZ service, no adapter, rfkill blocked (soft/hard), and present/powered + powered-off; each asserts visibility, status text and `timer_armed() == false`. **UNBUILT** | Tests not executed here (brief forbids builds/tests); orchestrator must run them. |

Precedent: `.vendor/cosmic-epoch/cosmic-applets/cosmic-applet-bluetooth/src/bluetooth.rs:99-111` requires a default adapter and yields no usable applet state without one (no retry timer adopted here, because the brief requires zero timers while absent); `.vendor/vela-main/bluetooth/src/rfkill.c:69-108` reads `soft`/`hard` from sysfs without polling, which is the rule copied for `adapter_rfkill_blocked`.

PASS — all three absence inputs (no BlueZ, no adapter, rfkill blocked) now map through one pure, DBus-free classifier to a hidden tray/popup row with no timer armed, while present/powered output is byte-identical. Two production files and four unit tests were edited; every edit is marked UNBUILT because the brief forbids builds and tests, so compile/test verification is deferred to the orchestrator. No timers or polling were introduced; the only Bluetooth FD is the existing event-driven D-Bus watch.
