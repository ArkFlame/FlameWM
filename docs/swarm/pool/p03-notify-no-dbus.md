# Unit p03-notify-no-dbus
STATUS: PARTIAL

Notification daemon / shell: when the D-Bus session bus is missing or the name is already owned, never panic or exit the shell; log one failure record with blast radius (no notifications) and continue.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/notifications.rs:253 | `start()` emitted its own `eprintln!` when `org.freedesktop.Notifications` was already owned, while a missing session bus was logged only in the shell; no single failure record existed. | Removed the provider-side `eprintln!`; state still becomes `ProviderState::Unavailable` and `Ok(false)` is returned. UNBUILT | Low: return value and state unchanged. |
| crates/flamewm-shell/src/main.rs:117 | `NotificationHost::connect` returned `Ok(None)` on name-owned, so the shell boundary logged nothing. | Return `Err("org.freedesktop.Notifications is already owned by another daemon")` so the existing shell handler emits one record. UNBUILT | Low: resulting host is still `None`; message text only. |
| crates/flamewm-shell/src/main.rs:888 | Shell failure log did not state the blast radius required by this unit. | Log now reads `native notifications disabled (blast radius: no notifications): {error}`. UNBUILT | Low: log text only; no visual or control-flow change. |

Evidence: `NotificationProvider::connect()` (notifications.rs:168) propagates `BusPump::connect(BusKind::Session)` errors, and `start()` (notifications.rs:235) returns `FlameResult<bool>`; both failure modes are handled at main.rs:883-891 by logging and continuing with `None`, never panicking or exiting. All shell uses of the `notifications` host are `if let Some(host)` guarded (main.rs:1055, 1189, 1257); the nearby `.expect` at main.rs:1590 covers a static embedded template, not D-Bus. Reference-first: `.vendor/**` contains no notification-daemon implementation, so the local degradation pattern (matching launch feedback, polkit, and bluetooth agents in the same startup function) was followed. Cargo build and tests were not run because the brief forbids builds; every edit is UNBUILT.

PARTIAL — the three edits consolidate both D-Bus failure paths into one shell-level record with an explicit "no notifications" blast radius and preserve existing graceful degradation; correctness is unverified because builds/tests were forbidden, and the log wording is the only behavior change.
