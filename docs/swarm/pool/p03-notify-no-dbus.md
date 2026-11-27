# Unit p03-notify-no-dbus
STATUS: PARTIAL

Notification daemon / shell: when the D-Bus session bus is missing or the name is already owned, never panic or exit the shell; log one failure record with blast radius (no notifications) and continue.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/notifications.rs:253 | `start()` emitted its own `eprintln!` when `org.freedesktop.Notifications` was already owned, so the shell would have written a second line for the same failure. | Removed the provider-side `eprintln!`; state still becomes `ProviderState::Unavailable` and `Ok(false)` is returned. UNBUILT | Low: return value and state unchanged; a standalone provider caller no longer sees that console line. |
| crates/flamewm-shell/src/main.rs:108-168 | No typed failure record existed for notification startup. A missing session bus propagated `Err` to `main` (which exits on `execute()` failure) and the name-owned branch returned `Err` too, so neither path produced a record with a blast radius. | Added `record_notifications_unavailable` using `flamewm_session_core::diagnostics` (`FailureCode::SubsystemError`, `BlastRadius::new(["session","shell","wm","panel","dock"], ["notifications","notification_center"])`, `fallback_taken = true`) and made all three D-Bus startup failures record once and return `Ok(None)`. UI setup errors still propagate `Err`. UNBUILT | Low: diagnostics-only append; a failed write is logged and ignored. No host, surface or visual behavior changes. |
| crates/flamewm-shell/src/main.rs:940 | Shell failure log did not state the blast radius required by this unit. | Log now reads `native notifications disabled (blast radius: no notifications): {error}` for the remaining non-D-Bus setup errors. UNBUILT | Low: log text only; no control-flow or visual change. |

Evidence: `NotificationProvider::connect()` (notifications.rs:168) propagates `BusPump::connect(BusKind::Session)` errors; `BusPump::connect` (flamewm-dbus-reactor/src/lib.rs:52) maps libdbus errors to `FlameResult` with no panic. `start()` (notifications.rs:235) returns `FlameResult<bool>` and already sets `ProviderState::Unavailable` on an owned name. All shell uses of the notification host are `if let Some(host)` guarded; the nearby `.expect` at main.rs:1590 covers a static embedded template, not D-Bus. The typed record follows the in-repo owner used by `flamewm-lock/src/pam.rs:88` and `flamewm-wm-wayland/src/main.rs:430`; `.vendor/**` has no notification-daemon implementation to borrow. Cargo build and tests were not run because the brief forbids builds, so every edit is UNBUILT.

PARTIAL — both D-Bus failure modes now produce exactly one typed failure record with an explicit "no notifications" blast radius and leave the shell running with a `None` notification host; the change is confined to diagnostics and the removal of a duplicate console line. Correctness is unverified because builds and tests were forbidden by the brief.
