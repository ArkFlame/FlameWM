# u18 — Notification daemon and lock screen idle wakeup audit

**Scope:** notification daemon (`flamewm-integrations-linux/src/notifications.rs`,
`notification_history.rs`) and `crates/flamewm-lock/src`; plus this file. Read-only audit of
polling loops, fixed sleeps, unconditional redraws, clock-tick repaints and history bounds, with
safe cuts that do not change rendered output. RULES: no `git`, no `cargo`, no Python, no sleep
loops, no builds or tests; shared checkout, orchestrator commits. Every code edit below is
**UNBUILT** (no compiler verification was permitted in this pass).

## Findings

| # | Area | File:line | Cost | Change | Risk | Status |
|---|------|-----------|------|--------|------|--------|
| 1 | Notification expiry | `notifications.rs:326-346` (`expire`, called from `on_ready:298-299`) | Every D-Bus readiness turn scanned all active notifications and allocated a `Vec` of ids, although expiry is due only at the earliest toast deadline. | Added an early return when `next_deadline()` is absent or still in the future; due expiries still run the original close/signal path. | Low — `next_deadline` is the minimum of the same deadlines the scan filters, so the predicate is identical for due entries; count stays 0 when nothing is due. | UNBUILT |
| 2 | Notification history bounds | `notification_history.rs:10-11,57,110-130,243-244`, `notifications.rs:24-25,30-31,408-416,616-632` | Checked: history is capped at 200 entries, 256 KiB of text and 256 muted apps; the persisted file is capped at 512 KiB; decoded images are dropped before retention; active toasts cap at 64 and UI updates coalesce to a `Reset` at 256. No unbounded growth at idle. | None. | None — observation only. | UNBUILT (no edit) |
| 3 | Notification expiry scheduling | `flamewm-shell/src/main.rs:641-643,718-726` (read-only, not owned) | Checked: the consumer calls `expire` only when a deadline is due and publishes `next_deadline()` as a reactor wake, so toast closure is event-driven and needs no polling tick. | None. | None — observation only; owned crates unchanged. | UNBUILT (no edit) |
| 4 | Lock clock repaint gate | `run/paint.rs:38-63`, `run/tests.rs:204-210` | Checked: `PaintKey.time` is minute-resolution `LocalTime`; `repaint` returns before projection when the key matches, and a second `view_changed`/`power_changed` check avoids redraw when the projected content is equal. No unconditional redraw on a 1 Hz tick. | None. | None — observation only. | UNBUILT (no edit) |
| 5 | Lock hidden date formatting | `view.rs:35-41` (`LockView::project`) | A minute-boundary repaint formatted the full date string and wrote it to the hidden date node even with `lockShowDate` off, allocating a `String` once a minute. | Format the date only when `settings.show_date`; otherwise pass an empty `String` to the same `apply_view` write. | Low — the affected node is invisible; enabling the flag bumps `settings_revision` and takes the full-sync repaint that restores the text. | UNBUILT |
| 6 | Frame pacer wake after animation stop | `run.rs:326-373` (`FramePacer::start`, change at 353-360) | The fixed `thread::sleep(FRAME_INTERVAL)` always wrote a wake byte after sleeping, even when the Ribbons animation had been stopped during that sleep, causing one needless reactor turn. | Re-check the running flag after the sleep; when it is off, loop back to the condvar wait without waking. The 33 ms cadence while running is unchanged. | Low — `set_running(true/false)` still controls the condvar; a stop during the sleep now suppresses only the stale wake, and a restart before the check still produces a normal frame. | UNBUILT |
| 7 | Lock idle tick | `run.rs:63-71,238-240`, `screensaver.rs:42-68`, `run/tick.rs:91-99` | Checked: the single 1 s repeating timer covers minute-granularity screensaver thresholds, the minute clock, grab/stacking safety nets and DPMS settling. Keys, pointer, control snapshots and worker results wake the reactor directly through existing fds and `Waker`. Lowering the rate would delay visible screensaver/clock transitions, so it was left intact. | None. | None — observation only. | UNBUILT (no edit) |
| 8 | Settings preview polling | `run/preview_mode.rs:66-108`, `preview.rs:41-53` | Checked: the 50 ms timer samples `XQueryPointer`/`XQueryKeymap`; those calls produce no fd readiness, so the short-lived preview must poll to see the first input. A Ribbons frame is written and redrawn only when `next_frame` is due. | None. | None — observation only. | UNBUILT (no edit) |
| 9 | Failed restore / DPMS retries | `run/idle.rs:50-60,85-98`, `run/input.rs:8-30` | Checked: `retry_background_restore` and `settle_display` run only while a `background_stale` / `display_on_pending` flag is set, at most once per tick; the immediate restore on wake is event-driven. Neither is an unconditional loop. | None. | None — observation only. | UNBUILT (no edit) |
| 10 | Lock cross-thread wakeups | `wake.rs:8-32`, `run.rs:164-168,228-237` | Checked: PAM, power and frame-pacer results arrive on a watched socket pair as single nonblocking bytes; a full socket already means a wake is pending. No worker polling loop. | None. | None — observation only. | UNBUILT (no edit) |

PARTIAL — Three safe cuts landed: notification expiry now skips scanning/allocating until a toast
deadline is due, the lock skips formatting a hidden date, and the frame pacer no longer wakes the
reactor after the animation stops. The remaining 1 s lock tick and 50 ms preview poll are intentional:
they are the only way to observe minute-based screensaver thresholds and X pointer/keymap state
without a visibility change, so they were audited and left alone. No build or test was permitted;
all edited code is UNBUILT and needs the orchestrator's verification pass.
