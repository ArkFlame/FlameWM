# LOG u113-review-batch17 — regression review of the newest batch

Batch eighteen. Owned paths: `crates/**/src`. `cargo` and `git` are forbidden in the shared
checkout, so the reviewed set is the units that landed after the previous review (`u109`, which
covered `u69`–`u108`): `u110` (refresh debounce), `u111` (worker-death error paths), `u112` (tests),
plus the two code fixes `u109` itself landed (`F1` Wayland commit gate, `F2` transfer conflict probe).
`u114`–`u117` were still being written during this review (their files have later mtimes and no
landed log), so they are out of scope here.

Every statement below is a source read; the orchestrator builds and runs the tests.

## Method

For each changed mechanism I read the current source, not the unit log's claim, and checked the four
regression classes: dropped wakeups, missed redraws after state change, races on worker threads, and
lost cancel paths. Test-only changes were checked for semantic correctness against the code under
test (the orchestrator can run them, I cannot).

## Result — no regression requiring a source edit

### u110 — explorer directory debounce (`refresh.rs`)

Sound. Trailing edge restarts `pending` on every drained batch and stamps `burst_start` only on the
first batch of a burst, so `poll_timeout_ms` (min of quiet and max-wait) cannot defer past 1 s or fire
early. Both deadlines are cleared together on fire, so a following burst gets a fresh window; the
final drained event always fires because the quiet deadline only moves when events are drained. An
idle watch passes `None` to `poll` and sleeps with no timeout (zero idle wakeups). `read_events`
returns `WouldBlock` on the nonblocking inotify fd, so the drain terminates. Test arithmetic verified
against the helpers: `poll_timeout_ms(Some(t0+900ms), Some(t0), t0+900ms)` is 100 ms + 1 = 101 ms.

### u111 — settings and icon wake fds (`shell/main.rs`, `icon_loader.rs`, `app/icons.rs`)

Sound. The startup-settings source is one-shot: its writer drops at worker exit (including a refused
spawn, whose closure is dropped with the error), so the read yields `Ok(0)` and the handler returns
`FdAction::Remove` after the first wake — no permanently readable fd. The icon wake path retires only
on EOF (`wake_drain` returns `false`); `ensure_loader` never resets a dead loader to `None`, so the
retired fd cannot be re-registered in a loop. A disconnected request or retry channel removes the
pending key instead of leaving a placeholder waiting, and `flush_theme` consumes `theme_outbox` even
on `Disconnected`, so `drain_ready`'s steady-state guard (`pending`, `retry`, `theme_outbox`,
`replies_outstanding` all clear) is reachable — no per-tick retry spin. `JobPool::submit` runs the
job on a fresh thread when every lane is gone; a job that panics cannot poison the queue mutex
because the guard is dropped before `job()` runs. Optional spawns (profiler, heap trim, settings
fetch, glide ticker) degrade; the required worker spawns (scan, thumb, io) still propagate their
error with `?`.

### u111 — task-manager sampler death (`task-manager/src/lib.rs`)

Sound, and the "sampling stopped" status line is actually reachable: the render loop calls
`on_reactor` unconditionally after `reactor.dispatch` (`render-x11/src/app/events/window.rs:45`), so
the EOF wake that retires the drain fd still runs the tick, applies the last queued snapshot
(`sync_channel(1)` backpressure plus wake-after-send keeps the newest) and sets the one-shot message
once. Action results are not overwritten unless another wake follows death.

### u111 — extraction timeout (`explorer/src/dndext.rs`) and child reaper (`terminal/src/links.rs`)

Sound. The pidfd wait is bounded by `min(deadline - now, CANCEL_POLL)`, so cancellation is still
observed while the child runs; `try_wait` reaps after every wake; a refused pipe-drain thread drops
the pipe so the child sees EPIPE and the outcome reports failure instead of hanging. The reaper
fallback spawns a per-child waiter when the shared reaper thread was refused.

### u109 F1 — Wayland commit gate (`wm-wayland/src/handlers.rs`)

Compiles against smithay 0.7 as used: `with_surface_tree_upward` takes `filter -> TraversalAction`,
`processor`, `post_filter -> bool` (`smithay-0.7.0/src/wayland/compositor/mod.rs:335`), and the root
visits itself as its own child (`tree.rs:507-519`), so `child != surface` is true only for real
subsurfaces. Placement-only parent commits now repaint.

### u109 F2 — conflict probe (`explorer/src/app/transfers.rs`)

Sound. The row shortcut is gated on `!current.loading`; `Tab::loading` is a real field that resets to
`true` on navigation (`explorer/src/lib.rs:176,243`), so a streaming tab always falls back to the
per-source `symlink_metadata` probe.

### u112 — added tests

Semantics checked against the code they pin: grid `compose_row`/`place_runs`/`overlay`/`clip` match
the asserted run columns, cells, texts and clipped `Arm`; `Reporter` emits once in `new`, and
1024 B < 10 000 B step and 1 entry < 5 entry step emit nothing; zero totals make completion true so
the final snapshot bypasses the interval; 4 × 64 KiB equals `CACHE_BYTES` and the fifth insert
crosses it (evicting only the LRU), a cached failure costs zero bytes; `percent` is the saturating
busy delta over the saturating total delta. Thumbnail tests reach private `bytes`/`entries` from a
descendant module, which is allowed.

## Residual risks (verified, not changed)

- **R1 (subsurfaced clients paint on every commit).** `F1` treats any parent with a real child as
  paint-worthy, so a client that commits a static subsurface tree without damage (Chrome/GTK CSD
  resize paths) now schedules a paint it did not need. Closing it needs a per-surface shadow of the
  last committed placement/scale, which is bigger than this review's budget and needs runtime proof.
- **R2 (long burst rebuild rate).** `u110` deliberately rebuilds the listing once per `MAX_WAIT` (1 s)
  during a copy into the visible folder; a pure trailing edge would freeze the view. Intended.
- **R3 (single status slot).** In the task manager a sampler-death status line is replaced by any
  later action-result message and re-applied on the next wake; cosmetic, no state loss.

## Verification

No `cargo` or `git` executed (shared checkout). No source edits were needed, so there is nothing to
compile-check beyond the existing tree. Status: UNVERIFIED for build; tests and build are the
orchestrator's gate.
