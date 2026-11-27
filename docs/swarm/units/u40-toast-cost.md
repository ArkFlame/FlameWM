# Unit u40 — notification toast and popup render cost audit

Status: **UNBUILT** — source audit only. No `cargo`, build, test or runtime measurement was run in
this unit (worker constraint). Visual equivalence is argued from the unchanged projection path, not
measured.

Hotlist reference: `docs/PERF_HOTLIST.md` rank 10 (toast icon 15 ms / 66 Hz per toast; session
readiness 10 wakeups + 10 X11 connections per second during boot). Prior log entries:
`docs/perf-loop/LOG.md:212-215` (u158 bounded icon wait), `docs/perf-loop/LOG.md:102` (u146
event-driven readiness watch).

| file:line | wakeups | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/main.rs:169-171`, `:199` | 0 periodic (one idle worker + fd at startup) | Spawn the toast `IconLoader` at connect so its wake fd exists when reactor sources are registered; previously spawned lazily on the first named icon. | An extra idle thread and socket pair for the whole shell lifetime when no toast appears. No rendering change; spawn failure keeps the old fallback path. |
| `crates/flamewm-shell/src/main.rs:210-220` | 0 | Add `icon_wake_fd()` and `drain_icon_wake()` to `NotificationHost`; the fd callback drains wake bytes only. | Callback takes an immutable `RefCell` borrow outside the tick; reactor dispatch is single-threaded. EOF returns `false`, so the source is unregistered instead of spinning. |
| `crates/flamewm-shell/src/main.rs:1187-1206` | Replaces up to 66 wakeups/s per pending toast with one wake per finished raster | Register the toast loader's wake fd on the reactor; each arrival wakes the existing tick, which drains and applies the icon through the unchanged `set_icon` path. | A second raw-fd source is registered; the host owns the fd for the run loop's lifetime. If the worker dies, `FdAction::Remove` unregisters it and the fallback glyph stays. |
| `crates/flamewm-shell/src/main.rs:631-643` | 66 Hz -> 0 while the icon is in flight (event-wake path) | Drop the 15 ms deadline re-arm; keep `ICON_WAIT_LIMIT` (2 s) expiry and the bounded 15 ms poll only for a loader spawned after startup. | A lost answer now leaves `icon_wait` set until another wake, but the fallback glyph is already shown, so visuals are unchanged and no periodic timer runs. |
| `crates/flamewm-shell/src/main.rs:560-566` | 0 | A late loader (startup spawn failed) sets `icon_event_wake = false`, retaining the old bounded poll. | Rare fd/thread exhaustion path keeps the previous behavior; no visual change. |
| `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs:56-93`, `:189-265` | 0 periodic; one wake per property event or child/stop signal | No edit: u146 already keeps one `RustConnection` in `ReadinessWatch`, selects root `PROPERTY_CHANGE`, and blocks in `poll(2)` on that socket plus the wakeup pipe. | Unchanged. The earlier ten connections/s are already gone; no action needed. |
| `crates/flamewm-wm/src/bin/flamewm-session/readiness.rs:340-343` | Failure path only | No edit: `ewmh_ready()` is the no-watch fallback. Removing it could lose readiness when `change_window_attributes` fails but a plain connect works. | Left as a safety net; does not run in the normal watch path (`:213-219`). |

Boundaries: no edits to `notifications.rs`, `icon_service.rs`, `launcher_entry.rs`, `icon_loader.rs`,
`app/icons.rs` or other shell icon files; those remain owned by concurrent workers.

Conclusion: one safe cut applied (toast icon polling replaced by an event wake); the readiness
connection-reuse cut was already present from u146 and was verified by source inspection only.
Neither cut was compiled, tested, or measured; all claims here are static and the unit is UNBUILT.
