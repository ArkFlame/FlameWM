# u63-idle-desktop log

Batch nine over the idle-desktop composition paths: `crates/flamewm-desktop/src/**`,
`crates/flamewm-shell/src/**`, `crates/flamewm-reactor/src/**`. `git` and `cargo` are forbidden in
this shared checkout, so the review of earlier slices is source-based and the orchestrator builds.
`rustfmt --check --edition 2024` on every edited file is clean.

## Review of the landed slices

- Shell clock (`LOG-u46-shell3.md`): the reactor minute timer plus the shell's own epoch-aligned
  `clock_next_at` deadline still dedupe to one projection per wall second and stay `None` while the
  clock is hidden. No regression.
- Reactor (`LOG-u47-reactor3.md`): `next_repeat_deadline`, `bounded_timeout` and the no-clock-read
  idle dispatch are intact; `dispatch` still skips `Instant::now()` when no slot is armed.
- Desktop async ingress (`LOG-u53-desktop-async-audit.md`): wallpaper/icon/trash/downloads workers
  remain off-turn. One defect found in the wake ownership that landed with that slice, fixed below.
- Desktop widget sampler (`LOG-u22-desktop-wm.md`): the 1 Hz seconds-clock fast path, the 4 s
  sample cadence and the obscured suspend still gate correctly. No regression.

Defect: `StickyState` and the downloads `Organizer` shared one reactor `WakeDeadline`. Sticky's
`arm_persist` overwrote a nearer downloads settle deadline, and `take_due` cleared the slot with
`None`, dropping a pending download wake entirely until some unrelated event arrived. That is both
a lost-wakeup (a file can sit unmoved on an idle desktop) and a source of late settles.

## Changes

1. **Each desktop periodic owner gets its own wake slot** (`sticky.rs`, `downloads.rs`,
   `main.rs`). `attach_persist_wake` now takes `&mut Reactor` and claims an
   `owned_wake_deadline()`; `register_downloads` does the same. `Organizer::arm`/`disarm` no longer
   read back and min-merge "another owner's" deadline; they replace or clear their private slot.
   The desktop reactor now scans three independent owner slots (widgets, sticky, downloads) instead
   of one shared cell that owners overwrote. Composition shift: no dropped settle wakeups, and a
   sticky save can no longer delay a downloading file by up to the debounce window.

2. **Idle widget poll reads no clock** (`widgets/lifecycle.rs`). `poll` returns before
   `Instant::now()` when no sample deadline, no tick deadline and no media change is pending. A
   widget-free desktop (the default profile) then pays nothing per event flush; before, every flush
   paid a clock read and a `media_dirty` take.

3. **Sticky persist flush reads no clock while idle** (`sticky.rs`). `flush_persist_if_due` returns
   early unless a save is pending or armed, so the common flush (pointer motion, selections) skips
   `Instant::now()`.

4. **Disabled downloads organizer costs no clock read** (`main.rs`, `downloads.rs`). The organizer
   gained `is_enabled`; the flush only calls `tick` when the watch is installed, removing the clock
   read and the entry check for the default (disabled) configuration.

5. **Widget sync reads local wall time only when a widget shows it** (`widgets/render.rs`).
   `sync_widgets` previously called `chrono::Local::now()` on every sync, including a desktop with
   no Clock, Calendar or Custom widget. It now reads the wall clock only when `uses_wall_time`
   matches and substitutes a documented `NO_WALL_TIME` placeholder otherwise; the sync key still
   stores `None`, so the time-differs fast path is unchanged. Composition shift: no timezone lookup
   and no `Local` construction per 4 s sample (and per refresh) on non-clock desktops.

6. **Shell automatic location is gated by the settings cache** (`app.rs`, `app/settings.rs`).
   `tick_auto_location` ran on every shell turn (every pointer-motion frame) with a settings hash
   lookup, a clock read and a waker `Arc` clone, even though `autoLocationEnabled` is off by
   default. The enabled flag is now cached against `(settings_revision, has_settings)`; while it is
   off the turn returns immediately, and the one change turn still runs so a disabled host drops any
   in-flight lookup (preserving the enable-edge behaviour).

No visual or behavioural change: same clock cadence, same widget output, same download settle
policy, same location lookups when enabled.

## Remaining / not done here

- Shell `publish_wake` already folds weather, usage, clock, OSD, switcher, dock and bluetooth into
  one earliest-deadline write per tick; no further batching was needed there.
- The reactor minute timer is still registered unconditionally in the shell. It costs one wake per
  minute and is effectively redundant with the shell's own epoch-aligned clock deadline, but it also
  covers a hidden-clock realignment case; left in place rather than removing a behaviour owner.
