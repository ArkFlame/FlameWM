# z01 — Pointer motion hot path: zero allocation per event

Design doc for the `flamewm-wm-x11` pointer motion path. Implementation in this slice is
**step 1 only** (pure reusable-buffer reducer entry point + equivalence tests, UNBUILT).
The full plan is recorded here so later steps land as independent, compilable commits.

## Current structure (file:line)

Event arrival and coalescing (already allocation-free):

- `crates/flamewm-wm-x11/src/event_pump.rs:107-135` — `collapse_motion_run_result`. Contiguous
  `MotionNotify`s for the same `event` + `child` collapse to the latest by value; no `Vec`, no
  `String`. This is the X11-side motion compression.
- `crates/flamewm-wm-x11/src/wm/events.rs:63-66` — root-sourced motion is ignored; client motion
  goes to `handle_motion`.
- `crates/flamewm-wm-x11/src/wm/pointer.rs:313-419` — `handle_motion`. `resolve_frame_event_source`
  (`wm/mod.rs:796-798`), `snap_zones_at` (`wm/outputs.rs:303-309`), `snap_target_in_output`
  (`live_settings.rs:143`), `motion_repeats` (`pointer.rs:581-586`) and `RootPoint::new` are all
  stack/value work.
- `crates/flamewm-wm-x11/src/wm/trace.rs:5-56` — `emit_pointer_trace` builds its message inside a
  closure; `flamewm-debug/src/lib.rs:212-230` runs the closure only when tracing is enabled and not
  on cooldown, so a normal motion allocates zero trace bytes.
- `crates/flamewm-wm-wayland/src/input/**` — pointer input is value-based; the `Vec`/`String`
  hits found there are keyboard layout/settings initialization, not per-motion work.

The hot path (per accepted motion):

- `crates/flamewm-wm-x11/src/wm/effects.rs:463-572` — `reduce_and_execute`.
- `crates/flamewm-wm-x11/src/wm/interaction.rs:87-91` — `step_owned` consumes the controller, so
  no `ControllerState` clone on this path; `PointerCommit::plan` (`:31-54`) moves the effect plan
  into the commit.
- `crates/flamewm-wm-x11/src/frame/controller/reducer.rs:14-260` — `reduce_owned`.
- `crates/flamewm-wm-x11/src/frame/controller/planning.rs:175-249` — `move_step`.
- `crates/flamewm-wm-x11/src/frame/controller/planning.rs:100-108,84-95` — `move_effects` and
  `full_configure`.
- `crates/flamewm-wm-x11/src/frame/controller.rs:51-91` — `FrameEffect` is `Copy`, so an effect
  plan can live inline in a reused buffer without cloning.

## Counted allocations per `MotionNotify`

| # | Site | Count | Notes |
| --- | --- | --- | --- |
| 1 | `planning.rs:100-108` `move_effects` (via `move_step` tail `:243`) | **1 heap alloc per accepted move motion** | `vec![MoveFrameRoot, notify_root]`, dropped after `execute_effects`. Biggest recurring allocation. |
| 2 | `planning.rs:183,218,236` no-op returns in `move_step` | 1 heap alloc per suppressed motion | `vec![Noop]`; repeats are already filtered by `motion_repeats`, but clamped/noop motions still allocate. |
| 3 | `planning.rs:226-231` outline return | 1 heap alloc per outline motion | `vec![DragOutline]`. |
| 4 | `planning.rs:84-95` `full_configure` | 1 heap alloc | Only on first activation / final commit of a non-floating drag, not steady state. Already uses `Vec::with_capacity(6)`. |
| 5 | `trace.rs:42-55` | 0 when tracing disabled | Gated closure; excluded from the hot-path count. |
| 6 | `interaction.rs:100` `store_controller` | 0 after warm-up | `or_insert_with(|| next.clone())` runs only on the first event for a client. |

Net: a steady-state move drag performs **one to two heap allocations per `MotionNotify`** (items
1+2 or 1+3), even though the pump already compresses the event burst and the plan is at most three
`Copy` values. That is the allocation to remove.

## Target design

Keep one caller-owned `Vec<FrameEffect>` alive for the whole motion path. `clear()` it before each
step and fill it in place; retained capacity turns the per-event allocation into a single warm-up
allocation. The typed facade is the existing `FrameEffect` contract plus a function that takes
`&mut Vec<FrameEffect>` — no new dependency, no raw pointer, no `unsafe`.

- `move_effects_into(out, frame, extents)` — fills `MoveFrameRoot` + root notify; matches
  `move_effects` exactly (order and values).
- `move_step_into(state, session, pointer, final_commit, out) -> ControllerState` — reusable
  variant of `move_step`; identical branches, identical effect order, writes into `out`.
- `reduce_owned_into(state, event, out) -> ControllerState` — reusable variant of `reduce_owned`
  for the motion path; move motion uses `move_step_into`, everything else delegates to
  `reduce_owned` and moves its plan into `out`. Effects and ordering are unchanged, so pixels are
  unchanged.
- Hot-path callers will retain this buffer in `Wm` and, after `execute_effects`, move the plan back
  into the scratch so its capacity survives the next event (step 2). No visual state, settings, or
  effect semantics change at any step.

## Steps (each compiles alone)

- **Step 1 — reusable-buffer reducer entry point (implemented here).** Add
  `move_effects_into` and `move_step_into` in `planning.rs`, add `reduce_owned_into` in
  `reducer.rs`; keep `move_effects` / `move_step` / `reduce_owned` as delegating wrappers so every
  existing caller is unchanged. Tests: `move_effects_into` equivalence + capacity retention,
  `reduce_owned_into` equivalence for move motion and for a non-motion event. No caller rewired,
  no behavior or visual change. **UNBUILT** (no Cargo by worker rule).
- **Step 2 — own the scratch in `Wm`.** Add `motion_effects: Vec<FrameEffect>` to `Wm`
  (`wm/mod.rs`) and initialize it empty. In `wm/effects.rs::reduce_and_execute`, for
  `FrameEvent::Motion` in a Move session call `reduce_owned_into(..., &mut self.motion_effects)`,
  build `PointerCommit` from the taken buffer, and return the buffer into `self.motion_effects`
  after `execute_effects`. Compiles alone; removes the steady-state per-motion allocation.
- **Step 3 — extend reuse to resize/outline motion.** Add `resize_step_into` (and reuse the
  existing `configure_effects_into` at `planning.rs:39-59`) so contour drag motions also stop
  allocating. Verifies against `tests_effects.rs` order tests.
- **Step 4 — verification (outside this worker).** Build and run the crate tests with the
  orchestrator's Cargo owner, then scoped allocation-rate evidence for a 1 kHz synthetic motion
  stream. Not executed here.

## Reference and choice

- **Path:** `.vendor/xfwm4-master/src/moveresize.c:1055-1059` (`c->x/c->y` computed straight from
  `event->motion.x_root/y_root`) and `.vendor/xfwm4-master/src/display.c:700-702` (motion carried
  in a stack `XfwmEvent` union). Secondary: `.vendor/kwin-master/src/input_event.h` (input events
  are value/tagged types). No kwin *motion-compression* helper with equivalent semantics was found
  in the corpus; that part is uncertain.
- **Choice:** port xfwm4's *semantics* — apply the newest root coordinates directly through a
  stack/value plan, with no heap-allocated per-event effect list — not its C code or event union.
  FlameWM keeps its typed `FrameEffect`/`PointerCommit` boundary; the reusable buffer is a
  `Vec<FrameEffect>` filled in place. `.vendor/**` stays read-only reference, never a dependency.

## Rollback

Each step reverts independently. Step 1 rollback: remove `move_effects_into` / `move_step_into`
from `planning.rs`, `reduce_owned_into` from `reducer.rs`, and their two test modules; the wrapped
originals keep their original bodies. Steps 2-4 revert only their own field/call edits. No
settings schema, asset, visual state, dependency, or gate is touched at any step.
