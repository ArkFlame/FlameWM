# Unit p43-panic-boundary
STATUS: PARTIAL

Crates flamewm-shell, flamewm-desktop, flamewm-lock: wrap each subsystem thread entry in the session-core guard so a panic becomes a FailureRecord and a restart, not a dead session. List thread::spawn sites lacking it; fix up to 3.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-shell/src/icon_loader.rs:282 | `shell-icon-loader` long-lived `while let Ok(msg) = req_rx.recv()` worker had no unwind boundary; a panic killed themed icons for the session. | Wrapped the worker body in `flamewm_session_core::diagnostics::guard("shell-icons", move || …)`; an unwind now yields `Err(FailureRecord)`, the thread exits cleanly, channels disconnect and the shell keeps running without themed icons. | UNBUILT. No automatic respawn of the worker. Persistence of the record depends on the panic hook installed in the shell binary. |
| crates/flamewm-desktop/src/wallpaper_loader.rs:179 | `flamewm-wallpaper-loader` long-lived `wallpaper_worker` loop had no unwind boundary; a panic killed wallpaper loading. | Wrapped the worker body in `guard("wallpaper", move || …)`; an unwind yields `Err(FailureRecord)` and the loader closes instead of unwinding an unjoined thread. | UNBUILT. No automatic respawn. The desktop binary does not call `install_panic_hook`, so the record stays an in-memory `Err` unless a hook is installed at process start. |
| crates/flamewm-lock/src/run.rs:346 | `flamewm-lock-frames` condvar pacer loop had no unwind boundary; a panic ended the animation tick source. | Wrapped the pacer body in `guard("frame-pacer", move || …)`; `return` still exits the loop, while a panic yields `Err(FailureRecord)` and the lock loop stays alive. | UNBUILT. No automatic respawn. The lock binary does not call `install_panic_hook`, so the record stays an in-memory `Err`. |

Thread `spawn` sites in scope that still lack the guard (not changed; 3-edit cap reached):
- shell: `src/event_router/drag.rs:70` (short-lived app-shortcut worker), `src/main.rs:868` (settings worker), `src/weather_host.rs:110` (`spawn_worker` helper). Process `Command::spawn` sites in `assistant_popover.rs`, `quick_controls/supervisor.rs` and `weather_provider.rs` are not thread spawns.
- desktop: `src/input.rs:59`, `src/input.rs:99`, `src/input.rs:224`, `src/menu_actions.rs:129`, `src/files.rs:142`.
- lock: `src/power.rs:89`, `src/power.rs:108`, `src/session.rs:28`, `src/run/input.rs:77`, `src/wayland.rs:739` (`tests/wayland_unlock.rs:20` is test-only).

Reference precedent: `.vendor/smithay-master/src/backend/vulkan/mod.rs:704` wraps a native callback in `catch_unwind` so a Rust panic cannot cross the FFI boundary; the session-core `guard` applies the same boundary at the subsystem-thread entry.

PARTIAL: three long-lived subsystem thread entries now use the session-core unwind boundary, and all thread-spawn sites lacking it are listed above. This is partial because the guard only converts a panic into an in-memory `FailureRecord` and does not respawn the worker; `flamewm-desktop` and `flamewm-lock` also never install the panic hook that persists records to the session log. All three edits are UNBUILT by design (no cargo, no builds permitted in this role); the orchestrator must run build/tests. The release profile sets panic = abort, so catch_unwind cannot catch panics in release builds; the guard only takes effect in unwind builds. Visuals are untouched: no layout, styling, or frame-path code changed.
