# Unit p04-shell-services-degrade
STATUS: PARTIAL

crates/flamewm-shell and integrations-linux: when NetworkManager, bluez, upower or logind is absent, the tray items hide and the shell keeps running; find unwrap/expect/panic or retry-storms on those paths and fix the worst.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/power_profiles.rs:209-228 | `refresh()` is invoked on every battery-menu open (`lib.rs:253`) and on `start()`; with both power-profile names absent it sent a fresh `GetAll` per open, overwrote `pending_get_all`, and stranded the older ServiceUnknown reply so fallback/available=false lagged. | Early return when a `GetAll` is already in flight; one in-flight read per provider. UNBUILT | If a bus never answers a sent `GetAll`, later menu opens cannot resend until `stop()`/`start()`; D-Bus normally replies with ServiceUnknown. Static inspection only: no cargo/build/test run (worker rules). |
| crates/flamewm-integrations-linux/src/network_slot.rs:29-123 | Absent NetworkManager degrades to `Unavailable` with a dormant fd, no timer; production paths contain no unwrap/expect/panic. | none | none |
| crates/flamewm-integrations-linux/src/power.rs:166-321, crates/flamewm-integrations-linux/src/bluetooth.rs:400-497 | Absent UPower/BlueZ use dormant fds and typed error replies; unwrap/expect occurrences are in `#[cfg(test)]` blocks only. | none | none |
| crates/flamewm-integrations-linux/src/lib.rs:518-586 (session lookup) / polkit_agent.rs | logind lookup reports `NotFound`/`Unavailable` without a panic; no retry loop found on absent-env path. | none | none |
| crates/flamewm-shell/src/quick_controls/supervisor.rs:281-309 | Helper restarts bounded at `DEFAULT_MAX_RESTARTS` (3) and reaped via `try_wait`; no restart storm when services are missing. | none | none |

PARTIAL: one bounded-request fix applied to the absent-power-profiles path; NetworkManager, UPower, BlueZ and logind paths were audited and already degrade without production panics or retry storms, so no further edits were needed. Verification was not run because this worker is forbidden from cargo/build/test commands; the edit is UNBUILT and needs a coordinator build plus a private-bus test of repeated `refresh()` calls.
