# y09 — X11 event-loop coalescing: ordered motion and same-window configure collapse

## Current structure (file:line)

Line numbers are after step 1 landed.

- `crates/flamewm-wm-x11/src/event_pump.rs:1-8` — module doc names the owner of the buffered-drain collapse: contiguous same-target `MotionNotify` events collapse to the newest, contiguous same-window `ConfigureRequest` events merge, any other event stops the run and is stashed once. No reordering.
- `crates/flamewm-wm-x11/src/event_pump.rs:13-22` — `PumpedEvent { event, received, coalesced }`: one emission plus its raw-observed and dropped counts.
- `crates/flamewm-wm-x11/src/event_pump.rs:24-28` — `WmEventPump` holds exactly one `pending` lookahead slot, so a boundary event can never be crossed.
- `crates/flamewm-wm-x11/src/event_pump.rs:30-83` — `pump_next` (infallible convenience), `pump_next_result` (error-aware seam) and `next` (live-X wrapper over `conn.poll_for_event`). `pending.take()` is consumed before any fetch; on a fetch error nothing is stashed.
- `crates/flamewm-wm-x11/src/event_pump.rs:85-104` — `collapse_motions_result` dispatches on the seed type; every other event is emitted untouched with `received = 1`.
- `crates/flamewm-wm-x11/src/event_pump.rs:106-135` — `collapse_motion_run_result`: the latest same `(event, child)` motion wins; lookahead stashed once.
- `crates/flamewm-wm-x11/src/event_pump.rs:137-164` — `collapse_configure_run_result`: same-`window` requests merge; lookahead stashed once.
- `crates/flamewm-wm-x11/src/event_pump.rs:166-199` — `merge_configure_request`: later value wins per set axis, mask union, earlier value retained only for axes the later request did not set. Applying the merge is equivalent to applying both requests in order.
- `crates/flamewm-wm-x11/src/event_pump.rs:201-583` — 16 unit tests: motion burst, release/press boundaries, different window or child, pending exactly once, both error paths, configure axis merge, configure boundaries and both result-seam lookahead paths.
- `crates/flamewm-wm-x11/src/lib.rs:10` — `pub mod event_pump;`.
- `crates/flamewm-wm-x11/src/wm/mod.rs:196` — `use crate::event_pump::WmEventPump;`.
- `crates/flamewm-wm-x11/src/wm/event_loop.rs:13-36` — `drain_pumped_events` records `received`/`coalesced`, dispatches under the `wm.resize.motion.drain` profiler point, then records batch size and max batch.
- `crates/flamewm-wm-x11/src/wm/event_loop.rs:104-129` — two drains per reactor turn (before and after the hook) share one pump, so a stashed boundary event is emitted exactly once across both.
- `crates/flamewm-wm-x11/src/wm/counters.rs:36-80` — `ResizeCounterSet` and its `record_*` methods back the summary line; no new dependency.

## Counted problems

Counted against the pre-step-1 path, where every `poll_for_event` result was dispatched immediately.

1. **Unbounded redundant motion dispatches.** A drag burst of N contiguous motions for one target produced N dispatches, N geometry mutations and N paint passes for one physical pointer position. Count: N-1 redundant dispatches per burst, unbounded.
2. **Configure-request storms committed intermediate geometry.** A client resizing with several contiguous requests produced one commit each, so intermediate geometries could be flushed before the final one. Count: N-1 extra commits per burst, over up to 7 independently set axes (`x`, `y`, `width`, `height`, `border_width`, `sibling`, `stack_mode`).
3. **"Skip to the newest motion" reorders input.** Scanning arbitrarily through the queue for a later motion can apply a move after the physical button release. Count: 1 ordering hazard; the release boundary must never be crossed.
4. **An error path can duplicate or drop a lookahead.** If a transport error occurs mid-collapse, stashing the lookahead would replay or lose it. Count: 1 error-path hazard; the error branch must leave `pending` empty.

## Reference precedent (path + choice)

- `.vendor/RustWebRender-0.0.9/RUSTWEBRENDER_0.0.6_IMPLEMENTATION_REPORT.md:33-39` — the named reference, **"Ordered motion coalescing"**: keep the first motion as the interaction boundary, peek only the queue head, consume while the head is another motion for the same renderer window, and **stop immediately** at a release or any other event; `XEventsQueued(..., QueuedAfterReading)` brings socket-arrived input into the Xlib queue.
- `.vendor/xfwm4-master/src/events.c:2191-2195,2225-2227` — xfwm4 dispatches `MotionNotify` and `ConfigureRequest` one at a time in the same event filter. It confirms the problem shape but offers no coalescing helper to copy.
- `.vendor/icewm-master/src/movesize.cc:471-483,531-543` — incremental move/resize redraws only when geometry actually changed (`if (xx != ox || yy != oy)`): the same intent, redundant motion work suppressed at the effect boundary, by a different mechanism.
- Choice: port the **ordered-boundary rule only**, as a pure typed facade with no Xlib call and no vendor symbol. `WmEventPump` keeps one `pending` slot, collapses contiguous same-target motions to the newest, merges same-window configure requests axis-wise, and stops at the first other event; x11rb's `poll_for_event` replaces `XEventsQueued`. No vendor file, symbol or dependency is copied. Pixels stay identical because the coordinate and geometry finally applied equal the result of applying every event in order.

## Target design

- One `WmEventPump` per run loop, reused across both drains so a stashed boundary event is emitted exactly once.
- Collapse is **contiguous and ordered**: only same-target runs merge; a release, press, key, property or any other event ends the run and is delivered next.
- Motion: the newest same-target motion wins; its root/event coordinates, time and state are the ones applied.
- Configure: the later request wins per set axis, masks are unioned, and an axis set only by an earlier request keeps its value — equivalent to sequential application.
- The error-aware seam propagates transport failures without crossing a boundary or stashing a lookahead.
- `received`, `coalesced`, `committed` and `max_batch` are exposed by the existing counters and profiler point, with no new dependency.

## Steps (each compiles alone)

1. **Implemented here (UNBUILT):** add `crates/flamewm-wm-x11/src/event_pump.rs` with `PumpedEvent`, `WmEventPump`, the error-aware `pump_next_result` seam, contiguous motion collapse, same-window configure merge, the single-slot lookahead and the unit tests; export it at `lib.rs:10`; wire `drain_pumped_events` into both drains in `event_loop.rs` using the existing counters. This is the whole goal in one compiling step, with no new dependency and no visual change. Step 1 was already present in this shared checkout; this pass re-verified it by read and added the missing symmetric result-seam configure test at `event_pump.rs:555-582`.
2. **Planned:** add a non-blocking socket-input lookahead (x11rb has no `QueuedAfterReading` equivalent) so motions already arrived on the fd but not yet read join the burst. Compiles alone; changes only which available coordinate is targeted, never the release boundary.
3. **Planned:** apply the same collapse facade to contiguous `Expose` runs per window (`wm/events.rs:41`) so one repaint per window per turn survives. Compiles alone; the final paint is identical.
4. **Planned:** surface `received`/`coalesced`/`max_batch` in the existing resize summary emitter (`wm/counters.rs`, `wm/configure_summary.rs`). Compiles alone; reporting only.

## Rollback

- Step 1: delete `event_pump.rs`, remove `pub mod event_pump;` at `lib.rs:10`, remove `use crate::event_pump::WmEventPump;` at `wm/mod.rs:196`, and restore `event_loop.rs:13-36,104-129` to a direct `wm.handle_event(conn.poll_for_event()?)` loop without counters. No state or data migration; normal visuals return to the original per-event behaviour.
- Steps 2-4: each is one typed-facade change; drop the added function and its call, keeping step 1 intact.
- Every step leaves the finally applied coordinate and geometry identical to sequential application, so visuals stay pixel-identical, and adds no dependency.
