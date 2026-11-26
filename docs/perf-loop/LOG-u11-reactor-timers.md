# LOG u11-reactor-timers

Area: `crates/flamewm-reactor/src/**`, `crates/flamewm-dbus-reactor/src/**` (timeline, child_pipe, reactor dispatch helpers).

Composition reasoning: the reactor is woke by fd readiness already (calloop/epoll), so the remaining
share is (a) timer re-arm policy on every periodic wakeup, (b) per-frame animation math while
transitions run, and (c) syscalls on every child-process line the desktop writes.

## Changes

1. `flamewm-reactor/src/lib.rs` — repeating timers keep a fixed cadence.
   `register_timer(delay, repeat = true)` re-armed with `TimeoutAction::ToDuration(delay)`, which
   calloop resolves against `Instant::now()` at fire time: every callback's latency was folded into
   the next period (drift), and after a suspend each missed period fired back to back. A new
   `next_repeat_deadline` anchors on the first deadline and jumps to the next grid boundary, so a
   resume costs one wakeup instead of a catch-up burst, and long-running periodic work (profile
   reports, preview polls, desktop ticks) no longer drifts late over time.
   Zero-delay repeats keep the old behaviour.

2. `flamewm-reactor/src/timeline.rs` — cubic easing without `powi`.
   `Easing::apply` called `powi(3)` for ease-out/ease-in, i.e. a libm call per sampled transition
   per frame (up to `MAX_ACTIVE` transitions at 60 Hz). Replaced with explicit products of the same
   polynomial: identical curve, no call.

3. `flamewm-reactor/src/child_pipe.rs` — one write per child line.
   `ChildPipe::write_line` did `write_all(line)`, `write_all("\n")` and `flush()`. `ChildStdin` is
   unbuffered, so `flush` was a no-op and the line plus newline cost two writes. Lines under 512
   bytes are now staged into a stack buffer and written once; longer lines keep the two-write path
   instead of copying a large payload.

## Not changed

- `register_wall_clock` already recomputes its delay from wall time on every tick (no drift) and
  re-aligns after suspend; no change needed.
- `flamewm-dbus-reactor`: the pump is already fd-driven (`read_write(Some(ZERO))` + bounded message
  budget), `watch()` re-exposed per turn; no polling loop to remove. `own_name`'s blocking call is
  startup-only, and the blocking `send_with_reply` API is used outside the owned paths.
- fd sources stay in calloop `Level` mode: callbacks drain with nonblocking reads, and one-shot
  re-arm would need an extra `epoll_ctl` per event.

## Expected composition shift

Timer wakeups: fewer after late dispatch or suspend (one realigned wakeup instead of a burst).
Animation sampling: one less libm call per active transition per frame. Child-process IPC: one syscall
less per written line.
