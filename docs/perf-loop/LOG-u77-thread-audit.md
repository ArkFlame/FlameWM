# u77-thread-audit

Scenario: one second of steady use of the four audited components, split by thread mechanics —
how much of the sample is thread creation, parked-thread wakeups and channel crossings, versus
the actual work those threads do. Absolute CPU is not the measure; the question is which share of
a normal second is spent on threads that exist only because a request arrived.

## Audit (what the four areas actually contain)

- `flamewm-terminal`: 2 threads per tab (PTY reader + wake pacer) and one thread per opened link
  (`xdg-open` waiter). The pacer is a second parked thread woken once per output batch: on a
  scrolling or `yes`-style prompt the composition was, per batch, one channel send to the UI, one
  channel send that futex-wakes the pacer, and one wake byte. Idle: two blocked threads.
- `flamewm-explorer`: persistent scan, thumb, IO, mime, search workers (already one thread each).
  Leftovers were per-request spawns: every `spawn_background` (sidebar rebuild, `lsblk`/udisks
  device action, and the coalesced resize flush — one thread per deferred resize during a drag)
  and one thread per remote listing or remote download.
- `flamewm-shell`: weather host, icon loader, quick-control supervisor and calendar usage host are
  single persistent workers driven by their wake sockets; the only production spawn is the
  drag-out desktop-shortcut worker, which is one-shot per user action and returns a joinable
  handle. No idle polling loop found (the `sleep` calls in that crate are test fixtures).
- `flamewm-tray`: no thread creation at all; the watcher is event-driven.
  → No change was justified there; spawning work would only add risk.

## Changes (features and visuals unchanged)

- `crates/flamewm-terminal/src/session.rs`: the wake pacer thread is gone. The reader thread now
  waits in `poll(PTY)` instead of `read`, with the poll timeout set to the end of the current 16 ms
  wake window when output is owed a wake, and no timeout when nothing is pending. A tab is one
  parked thread instead of two, and each output batch costs one event send plus the frame wake
  instead of an extra futex wake of the pacer. Coalescing is preserved: the wake byte is written at
  most once per `FRAME`, immediately after an idle frame (keystroke echo is not delayed), and it
  covers every batch queued before it — the same rule the pacer implemented by draining its queue.
- `crates/flamewm-terminal/src/links.rs`: opening a link no longer spawns a waiter thread per
  click. One lazily created reaper thread (`flamewm-terminal-reap`) waits children from a channel;
  if its thread cannot start, or it has already exited, the child gets its own waiter, so no child
  is left unreaped.
- `crates/flamewm-explorer/src/workers.rs`: added `JobPool`, a fixed set of reused threads with one
  job queue (`submit` falls back to a plain spawn only if every worker is gone), plus two pools.
  `aux_pool()` has two lanes for short background jobs so a slow udisks call or a stale network
  mount cannot delay the resize flush behind it. `remote_pool()` has two lanes because remote
  listings and downloads stream until done, and one of each must be able to run at once.
- `crates/flamewm-explorer/src/app/lifecycle.rs`: `spawn_background` submits to `aux_pool()`
  instead of `std::thread::spawn`. A resize burst now costs a channel send per deferred flush, not
  a thread; sidebar rebuilds and device actions reuse the same lanes.
- `crates/flamewm-explorer/src/app/navigation.rs`: remote listing, remote download, host-key probe
  and host-key save forwarders are submitted to `remote_pool()` instead of spawning a thread each.
- `crates/flamewm-explorer/src/app/menus.rs`, `transfers.rs`, `convert.rs`, `navigation.rs`: the
  remaining short UI-action workers (properties read, recursive size, set-as-wallpaper, clipboard
  publish/read, default-app open, conversion) submit to `aux_pool()`. Recursive size and a
  conversion can run long, which is why the pool has two lanes rather than one.

## Expected composition shift

In the terminal, per-tab thread count halves and one parked-thread wake per output batch
disappears from the output path; for N tabs the idle footprint loses N threads and N channels.
In the explorer, thread creation leaves the resize path entirely (the only per-input spawn in the
audited areas) and remote actions stop spawning at all; steady-state thread count is now constant
from the first action. Shell and tray are unchanged because their thread story was already
single-worker, event-driven.

Considered and not changed: `refresh::DirWatch` spawns and joins one thread per directory visited.
It blocks in `poll` on an inotify fd plus a stop pipe (no idle wakeups) and lives only for the
current folder, so the spawn cost is one per navigation rather than one per input; a persistent
watcher would need watch retargeting and per-directory notify plumbing for that sub-millisecond
share, so it was left as is.
