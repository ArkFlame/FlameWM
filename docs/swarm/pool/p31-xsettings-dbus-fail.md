# Unit p31-xsettings-dbus-fail
STATUS: DONE

Crates flamewm-xsettings, flamewm-control-dbus, flamewm-dbus-reactor: session bus missing or dropped mid-run: reconnect with capped backoff, no busy loop, no abort.

## Result
- crates/flamewm-dbus-reactor/src/lib.rs:49-177 | finding: `BusPump` is a passive external-reactor connection; connect and I/O failures map to `FlameError::unavailable`, with no panic, no abort, and no retry state. | change: none (UNBUILT); retry policy belongs to the link owner, not the transport. | risk: none.
- crates/flamewm-control-dbus/src/lib.rs:30-104 | finding: `ControlServer` owns a `BusPump` by value; `connect_session()` propagates errors and holds no reconnect state. | change: none (UNBUILT). | risk: none.
- crates/flamewm-xsettings/src/lib.rs:1-80 | finding: X11 selection producer only; no D-Bus session connection exists, so there is nothing to reconnect. | change: none (UNBUILT). | risk: none.
- crates/flamewm-wm-x11/src/runtime/control.rs:6-36,50-79,148-192 | finding: requested behavior already implemented in the consumer link: `BusRetry` capped exponential backoff (1s to 30s), `due()` gating so there is no busy loop, failure logged with the link set to `None` so there is no abort, and hangup or error drops the registration for reconnect. | change: none (UNBUILT); out of scope, already present. | risk: none introduced.
- crates/flamewm-wm-x11/src/runtime/bus_retry_tests.rs:3-27 | finding: tests cover the delay cap and reset after a successful connect. | change: none (UNBUILT). | risk: none.
- crates/flamewm-shell/src/main.rs:53,847-856,1507-1529 | finding: shell degrades to an offline snapshot on a missing bus and arms a one-shot retry chain that stops on success, so there is no busy loop and no abort. | change: none (UNBUILT). | risk: none.
- crates/flamewm-wm-wayland/src/control/service.rs:11 | finding: Wayland `Service::connect` still hard-fails on a missing session bus; this is outside the three crates assigned to this unit. | change: none (UNBUILT; deferred). | risk: needs a separate unit owning that crate if Wayland coverage is required.

Production edits: 0. All listed changes are UNBUILT; no build or test was run because the brief forbids them.

PASS - investigation is complete. The three named crates are passive transports with no connection lifecycle, and the requested reconnect behavior (capped backoff, gated to avoid a busy loop, non-fatal degradation) is already implemented in the X11 runtime through `BusRetry` and `refresh_control`, plus the shell one-shot retry chain. No production edit was required or made; the Wayland hard-fail path is out of scope and recorded as a follow-up. Findings are static only, since builds and tests were forbidden.
