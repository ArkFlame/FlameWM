# LOG u110-debounce-audit — watchers and debounce, batch seventeen

Area audited: explorer inotify watcher (`crates/flamewm-explorer/src/refresh.rs`), settings reload
(`flamewm-settings*`), tray (`flamewm-tray/src`), clipboard (`flamewm-clipboard/src`, read-only
audit — outside the owned paths).

## Audit of the four named debounces

- **Explorer directory watch (`refresh.rs`) — fixed.** The helper thread blocked in `poll` with a
  quiet-period timeout, but the deadline was anchored with `pending.get_or_insert_with(..)`, i.e. it
  recorded the *first* event of a burst. The doc comment already promised "`notify` runs once the
  directory has been quiet for `QUIET`"; the code instead fired every `QUIET` while a burst ran, so a
  2 s extraction into the visible folder rebuilt the whole listing ten times. The final event was
  never lost, but it was far from the documented trailing edge.
- **Settings reload (`flamewm-settings`, `flamewm-settings-core`) — no timer to lose.** Reload is
  stamp-driven: `ApplicationSettings::stamp()` is a `stat` per watched path, and `xdg_cache` re-reads
  a page only when the stamp moved. There is no pending-timer state, so no flush gap. (`notify`
  debounce for settings persistence lives in `flamewm-platform/src/services/settings_persist.rs`,
  outside this unit's paths; audited by reading only.)
- **Tray (`flamewm-tray/src/watcher.rs`) — no debounce.** Item and menu traffic arrives as D-Bus
  signals (`BusWatch`); `pending` there is an in-flight serial map (`MAX_PENDING` budget), not a timer.
  Drains remove by serial, and `retain` drops entries only for dead items, so no queued reply is
  silently timed out.
- **Clipboard keeper (`keeper.rs`) — no debounce.** The thread blocks in `wait_for_event` (XFixes
  selection notify) and handles every event; `GET_TIMEOUT` bounds one target read, not a coalescing
  window. Nothing can be left pending.
- **Search batching (`search.rs`) — verified, no change.** `walk` flushes the partial batch at the end
  (`if !batch.is_empty() { sink(batch) }`), and the event is sent before the pending flag flips, so the
  last batch cannot be dropped as "already queued".

## Change

`refresh.rs` now runs a trailing-edge debounce with a liveness cap:

- every drained inotify batch sets `pending = Some(now)`, restarting the quiet window;
- the first batch of a burst also sets `burst_start`, so `MAX_WAIT` (1 s) bounds total deferral;
- `poll_timeout_ms` wakes at the earlier of `pending + QUIET` and `burst_start + MAX_WAIT`;
- the callback fires when `should_fire(pending, now) || burst_forced(burst_start, now)`, then both
  are cleared, so an idle watch still blocks with no timeout.

The cap exists because `MASK` includes `MODIFY`: a large copy into the visible directory emits
events continuously for the whole copy, and a pure trailing edge would freeze the listing (no new
files, no growing size) until it ended. One refresh per second during such a burst keeps the view
live while still collapsing the hundreds of events behind it — a deliberate exception to pure
trailing edge, favoring no feature loss.

A burst shorter than `QUIET` behaves exactly as before. No lost final event: the quiet deadline
only moves while events are actually drained, and the last drained batch always fires.

## Expected composition shift

Per refresh callback the explorer spends a `readdir` + one `stat` per entry + listing projection and
document rebuild. During a long burst in the viewed directory (archive extraction, copy, build output)
that callback previously ran every `QUIET` (200 ms); it now runs once after the burst ends, or at most
once per `MAX_WAIT` (1 s) while it continues — a long-burst explorer UI share of N builds drops to
1 (bounded) instead of every 200 ms, exactly as with the repo's scroll glide
(`RowGlide`/`ResizeGate`, which already settle 200 ms after the last event). Idle wakeups stay at
zero: with no pending work the poll blocks with no timeout.

## Tests

`refresh.rs` gains `a_burst_that_never_goes_quiet_repaints_every_max_wait`, which pins the pure rules
and the deadline choice: not forced before `MAX_WAIT`, forced at it, the poll timeout picks the
earlier of quiet and max-wait (900 ms into a burst it wakes in 101 ms, not 200 ms), and an idle watch
has no timeout. Existing tests remain valid: `coalescing_waits_for_quiet_period`, the sub-`QUIET`
burst (`a_burst_of_writes_fires_one_callback`), and the zero-wakeup idle test. No wall-clock test for
a real long burst: under a loaded shared checkout sleeps can stretch and make it flaky, so the loop
change is line-local and reviewed by reading.

No cargo/git executed in this unit (shared checkout).
