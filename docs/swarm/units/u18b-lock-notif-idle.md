# u18b — Lock screen and notification daemon idle wake audit (second pass)

**Scope:** `crates/flamewm-lock/src/run.rs` and
`crates/flamewm-integrations-linux/src/notifications.rs`, plus this file. Shared checkout;
orchestrator commits. RULES: no `git`, no `cargo`, no Python, no builds or tests. This pass is a
read-only audit of timers, polls, repaints while nothing changes, and clock repaint frequency when
seconds are not shown. Cross-crate modules (`run/tick.rs`, `run/paint.rs`, `run/idle.rs`,
`run/input.rs`, `view.rs`, `screensaver.rs`, `power.rs`, `notification_history.rs`,
`flamewm-shell/src/main.rs`) were read as evidence only; they are not owned here and were not
edited. **No code was edited in this pass.** Every statement about code is **UNBUILT** — no
compiler, build or runtime evidence was collected here.

## Findings

Each row states the cost that was examined, the change actually applied (none in this pass), and
the risk of the change that was considered. "Present from u18" means the earlier unit's edit was
re-read and confirmed in the current checkout; it was not re-applied here.

| Area | file:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| Notification expiry guard | `notifications.rs:325-352` (guard at `327-331`, called from `on_ready:298-299`) | Earlier versions scanned every active toast and allocated an id `Vec` on each D-Bus readiness turn. | None this pass. Guard present from u18 and confirmed by re-read: `expire` returns `Ok(0)` before the scan whenever `next_deadline()` is absent or still in the future. | Low — `next_deadline()` is the minimum of the very deadlines the scan filters, so the predicate is identical for every due entry. |
| Notification history persistence | `notifications.rs:216-232`, called at `321` | History save on every readiness turn would be idle disk I/O. | None. `persist_history` compares `history.revision()` to `persisted_revision` and returns before touching the path when unchanged. | None — no idle I/O remains. |
| Notification wake source | `notifications.rs:133-144`, `292-295` | A daemon-owned timer or poll would wake the reactor without D-Bus work. | None. The provider owns no timer and no poll: its only readiness source is `BusWatch` from `BusPump`, and the documented contract has consumers drive `expire` from `next_deadline()`. | None — purely event-driven. |
| Notification UI update queue | `notifications.rs:401-421`, `621-637` | Unbounded update growth during idle. | None. Updates are bounded by `MAX_UI_UPDATES` and coalesce to a `Reset`; active toasts are bounded by `MAX_NOTIFICATIONS`. | None — bounded. |
| Lock clock paint key | `run.rs:264-274` (`PaintKey.time`), evidence `run/paint.rs:38-63`, `native_time.rs:6-14,40-59`, `clock.rs:22-36` | Repainting faster than the displayed minute when seconds are not shown would redraw the whole lock frame every second. | None. `LocalTime` has no seconds field, so `PaintKey.time` changes only on a minute boundary; `repaint` returns before projection when the key matches, and a second `view_changed`/`power_changed` equality check suppresses redraw when the projected content is equal. `format_clock` renders hour and minute only. | None — already minute-granular; no cut exists that preserves identical visuals. |
| Lock idle tick | `run.rs:63-71,238-240` | One repeating 1 s timer wakes the reactor even when no state changes. | None. The timer is the only driver for four sub-minute needs: the 4 s power error TTL, the 1 s grab/stacking safety net (`HOUSEKEEPING`), DPMS-on retries, and minute-granularity screensaver thresholds and clock. Keys, pointer, control snapshots and worker results arrive through their own fds and `Waker`. Lengthening `TICK` would delay visible clock and screensaver transitions and weaken the grab safety net; shortening it adds wakes without new visible state. | Visual/security — rejected as unsafe under the identical-visuals rule. |
| Frame pacer idle wake | `run.rs:326-379` (recheck at `354-359`) | A fixed sleep that always wrote a wake byte produced one needless reactor turn after Ribbons stopped. | None this pass. Post-sleep flag recheck present from u18 and confirmed by re-read: when the animation stopped during the sleep the thread returns to the condvar without waking; cadence while running is unchanged. | Low — already idle-silent; `set_running` still controls the condvar, so a restart before the check keeps a normal frame. |
| Lock cross-thread wake socket | `run.rs:164-168,228-237` | A worker-side polling loop would burn CPU at idle. | None. PAM, power and frame-pacer results arrive as bytes on a nonblocking socket pair; the reactor drains all pending bytes per readiness and a full socket already implies a pending wake. | None — event-driven, no worker poll. |
| Power error tick wake | evidence `run/tick.rs:53-56`, `run/input.rs:57-70`, `power.rs:259-267` | Feeding `PowerEvent::Tick` every second could bump the paint revision and redraw with no visible change. | None. `power_event` bumps `power_revision` on a tick only while an error message is actually shown; even then `repaint` finds `view_changed` and `power_changed` both false and returns before any `redraw`. | None — no redraw while nothing changes; avoiding the projection would require duplicating the power reducer's TTL state outside its owner, which is unsafe here. |
| Preview poll constant | `run.rs:71` (`PREVIEW_POLL`) | A 50 ms poll is a short-interval wake. | None. The constant is used only by the short-lived settings preview in `run/preview_mode.rs` (not owned). `XQueryPointer`/`XQueryKeymap` produce no fd readiness, so the preview must poll to see first input; a Ribbons frame is written only when `next_frame` is due. | Visual — removing it would freeze preview dismissal; outside this unit's ownership. |
| Screensaver, DPMS and restore retries | evidence `run/tick.rs:84-100`, `run/idle.rs:50-98`, `run/input.rs:8-30` | Unconditional retry loops at idle. | None. `settle_display` and `retry_background_restore` run only while `display_on_pending` / `background_stale` are set, at most once per tick; the immediate restore on wake is event-driven. Housekeeping is gated by `HOUSEKEEPING`. | None — flag-gated, not idle loops. |

## Applied edits

None. No file owned by this unit was modified in this pass; the two earlier cuts that touch the
owned files (`notifications.rs` expiry guard and `run.rs` frame-pacer recheck) were re-read and left
as they are.

## Verification

- No `cargo`, `git`, Python, build, test or sleep-loop command was run, per the brief. All findings
  are static source inspection only.
- Re-read immediately before finalizing: `run.rs:63-74,228-242,326-379` and
  `notifications.rs:292-353`; both matched the earlier read, so no concurrent edit needed to be
  accounted for.
- **UNBUILT:** there is no new code to compile; the pre-existing cuts in the owned files remain
  unbuilt in this checkout and need the orchestrator's build and test pass.

PARTIAL — the audit covered every timer, poll, repaint gate and clock path in the two owned files
and confirmed that the earlier expiry guard and frame-pacer recheck are present. No further cut
qualified as both safe and visually identical: the only remaining periodic wake is the 1 s lock
tick, which drives the 4 s error TTL, the 1 s grab/stacking safety net, DPMS retries and the minute
clock, and the 50 ms preview poll is active-only and short-lived. Zero code edits were made, so
there is nothing new to build; the two inherited edits stay UNBUILT pending orchestrator
verification.

Status: PASS
