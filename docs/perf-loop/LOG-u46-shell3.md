# u46-shell3 log

Batch six over the shell crate (tray / quick-controls / clock / weather ticks, dock hover). `git` and `cargo` are
forbidden in this shared checkout, so the review of the earlier slices is source-based (the landed timer/wake,
tray, quick-control, dock and projection code plus `LOG-u06-shell-timers.md` / `LOG-u07-shell-icons-dock.md`), and no
build was run here: the orchestrator builds, tests and commits.

## Review of the landed slices

Checked the single shared wake slot (`publish_wake`/`earliest_wake`), the reactor wall-clock registration, the tray
tick's dirty flags, `sync_dock` idle bail-out and hover-scale writes, `sync_dock_label` change detection, OSD
coalescing and the quick-control supervisor poll. Two defects surfaced in the clock path:

- The shell registered the wall clock with `with_seconds = false` unconditionally, so `taskbarShowSeconds` only
  produced correct text until the next minute: the panel showed a frozen second between minute boundaries.
- The clock projection and the per-panel `redraw` ran on every reactor minute tick even with `taskbarShowClock` off.

## Next worst hotspots and what changed

- **Clock cadence and hidden clock (`app/timers.rs`, `app.rs`, `src/main.rs` untouched).** The minute reactor timer
  still owns minute boundaries and suspend realignment, but the shell now also publishes its own boundary deadline:
  `clock_next_at` re-arms to the next second when `taskbarShowSeconds` is on and to the next minute otherwise, and
  to `None` while `taskbarShowClock` is off. The delay comes from the reactor's `wall_clock_delay_ms` so epoch-aligned
  boundaries have one authority, plus 1 ms so a tick never paints the second/minute that is ending. Ticks preserve
  the existing dedupe rule (one panel clock projection per wall second, forced on time-zone/settle/settings changes),
  so the reactor timer and the shell deadline landing in the same second paint once. Expected shift: seconds mode
  becomes correct at 1 Hz; a hidden clock does no formatting and no panel redraw per minute. Heap trim keeps its
  10-minute gate so seconds mode does not release the free heap every second.
- **Weather refresh had no wake (`app/timers.rs`, `weather_host.rs`).** The scheduler only fetched when some other
  event ticked the shell, so an idle desktop could keep a stale forecast for hours after
  `WEATHER_REFRESH_INTERVAL`. `WeatherHost::next_deadline` returns the 30-minute refresh instant when enabled with a
  location, `now` for a pending open fetch, and `None` while disabled/unconfigured or with a fetch in flight (the
  worker's result socket wakes the loop then). `publish_wake` now includes it: weather refreshes on time with one
  wake per interval and still does nothing while hidden/disabled.
- **Dock hover hit-test allocation (`app/dock.rs`).** Every `ActionPhase::Hover` event on the dock collected all
  slot rects into a fresh `Vec<(f32, f32, f32, f32)>` before `dock::hover_slot`. The same containment test now scans
  `DockFrame::slots` in place: pointer motion is the bulk of the event stream, so hover costs no heap traffic.
- **Idle dock settings re-parse (`app/dock.rs`, `src/dock.rs`).** `sync_dock` built the full `DockSettings` at the
  top of every shell turn, including a `Vec<String>` of pinned app ids and their clones, before the idle bail-out.
  New `dock_enabled` / `dock_auto_hide` peeks read the two single booleans the early returns need; the allocating
  settings pass now runs only for a real rebuild. `dock_hover_scale` and `dock_launch_bounce` replace the same
  allocating pass on every hover transition and dock click.
- **Quick-control pipe read per shell turn (`quick_controls/supervisor.rs`).** `drain_open_responses` read the
  helper pipe on every tick even when no `OPEN` was outstanding and no partial line was buffered. It now returns
  before the `read` syscall in that steady state; child-exit reaping (`try_wait`) and partial-line draining are
  unchanged, so a response is still consumed as soon as an `OPEN` is pending.

## Not done here

- The `flamewm-desktop`, `flamewm-explorer` and `flamewm-settings` unused-import/dead-function warnings named in the
  brief are outside this unit's owned paths (`crates/flamewm-shell/src/**`); they stay for their owning units.
- The reactor minute timer itself still fires once a minute while the clock is hidden; disabling that source needs
  reactor-side re-registration (u47), so only the projection/redraw work is skipped here.
