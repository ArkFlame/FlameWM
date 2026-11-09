# Reliability audit: WM and session crates

Scope: `flamewm-wm-x11`, `flamewm-wm-wayland`, `flamewm-session-core`, `flamewm-reactor`, production
code only. Method: a scripted scan of non-test code (`#[cfg(test)]` blocks and `tests*.rs` skipped) for
`unwrap`, `expect`, `panic!`/`unreachable!`, `as` casts, indexing, unbounded channels, child spawns and
poisoned locks, then a read of each hit in context. Base: origin/main `479b658f`.

Severity: High = can end the WM or compositor on a client or race; Medium = leak, hang or wrong
geometry on a reachable path; Low = invariant panic or no reachable path found.

## Findings

| file:line | risk | severity | fix |
|---|---|---|---|
| `crates/flamewm-wm-x11/src/wm/paint.rs:~291` (`refresh_titlebar_backdrop`) | Per-turn `paint_chrome(client)?` on a client destroyed before its `DestroyNotify` is pumped: `BadWindow` ends the WM | High | FIXED: `ignore_destroyed_window_race` per client |
| `crates/flamewm-wm-x11/src/wm/mod.rs` (`is_destroyed_window_race`) | `BadMatch` (e.g. `SetInputFocus` on a just-unviewable window) not treated as a race, so it ends the WM | High | FIXED: `ErrorKind::Match` added; tested |
| `crates/flamewm-wm-x11/src/wm/configure_handlers.rs:297-300` | `as i16`/`as u16` on client `_NET_MOVERESIZE_WINDOW` data: width 0 is `BadValue` (fatal); large values wrap | High | FIXED: `clamp_i16`/`clamp_u16` (min width 1); helper tested |
| `crates/flamewm-session-core/src/lib.rs` (`ProcessSupervisorPolicy`) | Restart limit was lifetime-total (5 ever), so a session that crashes rarely eventually gives up for good; no per-minute guard | Medium | FIXED: per-60 s window limit, capped exponential backoff, healthy minute resets window; `child_exited_at` tested with injected clock |
| `crates/flamewm-wm-wayland/src/handlers.rs:577` | `Mutex::lock().unwrap()` on xdg role data; a poisoned lock panics the compositor | Medium | FIXED: `unwrap_or_else(PoisonError::into_inner)` |
| `crates/flamewm-wm-x11/src/wm/events.rs:159-192`, `crates/flamewm-wm-x11/src/lib.rs:744` | Shortcut-spawned `Child` handles dropped unreaped: one zombie per press (finder, screenshot, terminal, `xdg-open`) | Medium | NOT FIXED (cross-scope). `flamewm-applications` already reaps via a private `spawn_reaped_with_id`; expose a `pub` wrapper there and call it here. Owner: applications |
| `crates/flamewm-wm-x11/src/lib.rs:781` (`perform_session_action`) | `systemctl suspend/reboot/poweroff` via blocking `.status()` on the WM thread, no timeout: hung D-Bus freezes the WM | Medium | NOT FIXED. Run on a worker thread with a bounded wait |
| `crates/flamewm-wm-wayland/src/probe.rs:301,461,471` | `systemctl`/`busctl` `.output()`/`.status()` with no timeout on the startup probe | Medium | NOT FIXED. Bound each call with a deadline thread or `timeout` |
| `crates/flamewm-wm-wayland/src/xwayland.rs:666` (`xwm_state`) | `expect` if an XWayland callback fires after `self.xwm = None` (`xwayland.rs:842`) | Medium/Low | NOT FIXED. Smithay trait returns `&mut`; needs a guard or a verified callback ordering |
| `crates/flamewm-wm-wayland/src/state.rs:1121` | `expect("every client carries compositor data")` on any client | Low | Invariant holds (smithay inserts data per client); no action |
| `crates/flamewm-wm-x11/src/runtime.rs:424` | `expect("host initialized")` after the guard is populated above it | Low | Invariant; no action |
| `crates/flamewm-wm-x11/src/wm/controllers.rs:54`, `wm/chrome_runtime.rs:101`, `crates/flamewm-wm-wayland/src/cursor.rs:162` | `expect` immediately after the value is populated | Low | Invariant; no action |
| `crates/flamewm-wm-x11/src/wm/drag_edge.rs:224,233` | `unreachable!` in edge-transfer arms | Low | Invariant; no action |
| `crates/flamewm-wm-wayland/src/session_lock.rs:66` | `expect` on session-lock global, created at startup | Low | Invariant; no action |
| `crates/flamewm-wm-x11/src/event_pump.rs:42` | `expect("infallible fetch cannot fail")` | Low | Invariant; no action |
| `crates/flamewm-wm-x11/src/wm/manage.rs:421,458-461` | `as i16`/`as u16` on computed placement (`max(1)` applied) | Low | Not changed; values are output-bounded |
| `crates/flamewm-wm-x11/src/runtime.rs:158` | `sync_channel(32)` for audio | Low | Already bounded; no action |
| `crates/flamewm-reactor/src` | All `unwrap`/`expect` sit in `#[cfg(test)]` | None | No action |

## Coverage gaps

- Slice indexing (about 177 scan hits) and `as` casts (about 300 hits) were scanned but not each read.
  Only the casts on client-controlled X data were reviewed.
- Client-fed `Vec`/map growth in the Wayland and X11 handlers was not measured.
- No runtime (Xvfb or nested) run was executed for these changes. Behaviour is covered by unit tests only.
