# x04 design: coalesce MotionNotify and ConfigureRequest per window

Route evidence: workspace version 0.1.7 (`Cargo.toml:6`; `scripts/version` reads the same value
through Python, which this brief forbids, so the manifest was read instead). Routes from
`.opencode/ROUTES.md`: source-first (owner, callers, dispatch order), rust-systems (ownership and
event order across the native boundary), x11-window-manager (ConfigureRequest authority and ICCCM
mask semantics), reference-mining (`.vendor/**` as evidence only), parallel-execution (one owner per
path in the shared checkout). Loaded skills were the five named above. No Git, Cargo, Python, build
or sleep command was run; all checks are static reads and `rg` searches.

## Current structure (file:line)

All coalescing code lives behind the `event_pump` facade of `crates/flamewm-wm-x11`.

- `src/event_pump.rs:13-22` — `PumpedEvent { event, received, coalesced }` and `WmEventPump`.
- `src/event_pump.rs:36-67` — `pump_next` / `pump_next_result`: pending lookahead, one collapse seam.
- `src/event_pump.rs:69-77` — `next`, the live `conn.poll_for_event` adapter.
- `src/event_pump.rs:90-104` — pre-change `collapse_motions_result`: motion-only dispatcher.
- `src/event_pump.rs:107-135` — `collapse_motion_run_result`: latest same-target motion wins.
- `src/event_pump.rs:138-164` — `collapse_configure_run_result` (added in step 1).
- `src/event_pump.rs:170-199` — `merge_configure_request`, the pure merge primitive (step 1).
- `src/event_pump.rs:315-449` — pre-existing motion and error-seam tests.
- `src/event_pump.rs:451-554` — configure coalescing tests added in step 1.
- `src/wm/event_loop.rs:15-36` — `drain_pumped_events`, generic over event kind, records counters.
- `src/wm/event_loop.rs:104-129` — `run_with_hook` drain points; pump created once per turn.
- `src/wm/events.rs:21-24` — dispatch: `Event::ConfigureRequest` goes straight to the owner.
- `src/wm/configure_handlers.rs:146-280` — `handle_configure_request`, one call per queued request.
- `src/wm/configure_handlers.rs:271-274` — accepted request: frame apply, configure notify, summary.
- `src/wm/counters.rs:36-55` — `ResizeCounterSet` (received/coalesced/committed/throttled).
- `src/wm/counters.rs:92-105` — `emit_resize_summary`, currently motion-oriented.

## Counted problems and gaps

- Coalescing coverage before this unit: **1** of the **2** named event kinds. `event_pump.rs` only
  collapsed `MotionNotify`; the pump's `collapse_motions_result` returned every other event
  untouched at its `let ... else` branch.
- Configure request handling before this unit: **1** full policy pass per queued request
  (`wm/events.rs:24` -> `wm/configure_handlers.rs:146-280`). For a burst of **N** same-window
  requests this produces **N** geometry plans, **N** `send_configure_notify` calls and **N**
  `emit_configure_summary` calls (`configure_handlers.rs:271`, `:273`, `:274`), although only the
  client's last desired geometry is observable on screen.
- Configure-specific coalescing tests before this unit: **0**. The test helper named `configure()`
  (`event_pump.rs:263-277`) built a `ConfigureNotify`, not a `ConfigureRequest`, so no test covered
  the request path or its value-mask semantics.
- Correctness hazard to avoid: **6** maskable fields on a `ConfigureRequest` (x, y, width, height,
  border width, sibling) plus stack mode. Keeping only the last request would silently drop an axis
  set by an earlier request; the merged request must union masks and keep the latest value per set
  axis.

## Reference precedent and choice

- `.vendor/icewm-master/src/yxapp.cc:1172-1237` — `handleXEvents` sets `prratio = 3`, advances
  `retrieved += prratio - 1` (`:1175`), processes a bounded subset of the queued burst, treats
  Button/Key/Motion as one ordered grab-eligible group (`:1206-1212`), and flushes X after each
  slice (`:1234`). **Choice:** port the semantic rule — collapse a queued burst to its latest
  intent, never cross a release or other boundary event — but not the fixed one-in-three skip.
  Fixed skipping can discard the final motion or request and change final geometry; keeping the
  latest value preserves it.
- `.vendor/xfwm4-master/src/device.c:574-611` and `src/events.c:2207-2264` — pull only relevant
  events with `XCheckIfEvent`/`XCheckMaskEvent`, translate XI2 to core motion once, dispatch one
  event per loop iteration, and gate cleanup on `XPending`. **Choice:** port the one-emission-per-
  step discipline with a single stashed lookahead event; FlameWM's `pending` slot already supplies
  that shape, so no event filter is copied.
- `.vendor/smithay-master/src/xwayland/xwm/mod.rs:1752-1784` — `Event::ConfigureRequest` is consumed
  with mask-gated `Option` fields for X/Y/WIDTH/HEIGHT/STACK_MODE. **Choice:** confirm that an unset
  value-mask bit means "not requested"; therefore a merge must union masks and carry the earlier
  value only for bits the later request did not set. No Smithay code was transplanted.
- `.vendor/kwin-master` was searched for motion compression but no matching local source was used;
  no code or dependency was taken from it.

## Target design

`WmEventPump` stays the sole owner of buffered-drain collapse and keeps its typed `PumpedEvent`
output. A run begins with a seed event; motion runs keep the latest same-target motion, configure
runs fold same-window requests through `merge_configure_request`, and any other event is emitted
untouched after the run. The first non-matching event is stashed once in `pending` and returned by
the next `pump_next_result` call, so release, press, key, map and destroy ordering never changes.
The merge is a pure function: later wins per set bit, masks are unioned, and parent/window/sequence
come from the latest request. Because a ConfigureRequest is absolute (not relative) geometry, the
merged request is equivalent to applying the burst in order, so the final frame geometry and pixels
are identical. The event loop drain stays generic; telemetry that distinguishes motion from
configure coalescing is deferred to a later step.

## Steps (each compiles alone)

1. **This unit.** Extend the pure pump to collapse contiguous same-window `ConfigureRequest` runs
   with mask-union/latest-value merge, preserve the pending slot and error path, and add six
   deterministic unit tests. Public signatures are unchanged; `event_loop.rs` and `wm/events.rs` are
   untouched. Behavior change is limited to which requests the existing handler sees. UNBUILT here.
2. Split `PumpedEvent` metadata into motion and configure counts (or add a kind tag), update the
   `drain_pumped_events` counter recording at `wm/event_loop.rs:20-31`, and add a scoped test.
   Compiles alone on the step 1 base; no visual change.
3. Add a dedicated configure-coalesce counter to `ResizeCounterSet` / `emit_resize_summary` and
   record retained perf evidence before making any further scheduling change. Compiles alone;
   visuals remain pixel-identical because only accounting changes.

## Rollback

Step 1 reverts by restoring `src/event_pump.rs` from the pre-change file: the dispatcher returns to
a single motion-only `collapse_motions_result`, the two run helpers and `merge_configure_request`
are removed, imports return to `x11rb::protocol::Event`, and the six new tests are deleted. The new
helpers are private to that module, so no other file imports them and no other module needs a
change. Steps 2 and 3 revert independently by restoring the `PumpedEvent` fields and the counter
set. No dependency or manifest was added.

## Step 1 result (UNBUILT)

One production file changed (`src/event_pump.rs`), with tests in the same file. All edits are
**UNBUILT**: no Cargo, test, Git or build command was permitted by this brief. The orchestrator
must run the workspace test gate before treating the step as verified.
