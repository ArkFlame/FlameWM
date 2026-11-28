# Unit p96-reduce-alloc-audit
STATUS: DONE (read-only audit; UNBUILT, no builds, no tests run)

p94 row 8: allocation cost per pointer motion in the X11 `reduce` path (crates/flamewm-wm-x11, interaction/ and frame/) is UNVERIFIED. Read the per-motion path from event pump to PointerCommit::plan; list every Vec/String/clone/format! per event with file:line. Propose at most 2 allocation-free replacements (reuse buffers). UNBUILT, no builds.

## Result

Scope note: there is no `interaction/` directory. The pointer commit lives in `crates/flamewm-wm-x11/src/wm/interaction.rs`; the motion reducer lives in `src/frame/controller/reducer.rs` and `src/frame/controller/planning.rs`.

### Per-motion path (live, after the drain)

1. `src/event_pump.rs:60` `WmEventPump::pump_next_result` calls `collapse_motions_result` (`event_pump.rs:86-121`). Heap allocations: none. `pending_slot` is an `Option<Event>` on the stack, and the drain closure is a `&mut dyn FnMut`.
2. `src/wm/effects.rs:488-496` `is_move_event` check and `move_counters().record_received(1)`. Not read in full, so heap use is UNVERIFIED. `flamewm_profiler::start("wm.move.plan_or_reduce")` at `effects.rs:499` is also UNVERIFIED (it is only a guard, but may allocate).
3. `src/wm/effects.rs:503` `interaction::step_owned(controller, event)` (no state clone on this path). This calls `reduce_owned` then `PointerCommit::plan`.
4. `src/frame/controller/reducer.rs:112-118` `FrameEvent::Motion` dispatches to `move_step` or `resize_step`. Neither clones state. The `state.session` match copies the session by value.
5. `src/frame/controller/planning.rs:145-219` `move_step`:
   - `:153`, `:188`, `:206` `vec![FrameEffect::Noop]` (1 heap alloc each).
   - `:159-165` `request(&state, ...)` borrows `state.outputs.as_slice()`; no alloc.
   - `:198-200` `vec![FrameEffect::DragOutline{..}]` (1 alloc, preview mode).
   - `:213` `move_effects` (`planning.rs:70-78`): `vec!` with 2 elements (1 alloc).
   - `:216` `effects.push(PaintChrome)` onto a len-2 cap-2 Vec, so it reallocates to cap 4 (1 more alloc). This fires on every moved frame.
   - `:178` `full_configure` (first activation only): `configure_effects` vec of 3, then `extend` of 3 more, so a realloc.
6. `src/frame/controller/planning.rs:221-281` `resize_step`:
   - `:231`, `:244`, `:268` `vec![Noop]` (1 alloc each).
   - `:261-265` `vec![DragOutline]` (1 alloc, preview mode).
   - `:273` `configure_effects` (vec of 3, 1 alloc), `:277` `extend([PaintChrome, ApplyShape])` (grows to 5, realloc).
   - `:271` `full_configure` on release only.
7. `planning.rs:35-52` `configure_effects`: `vec!` of 3 (1 alloc). `notify_root` is stack only.
8. `planning.rs:81-87` `release_effects` (release only): `push(Ungrab)` may realloc; `release_tail` (`:90-99`) `insert(0, ..)` shifts (release only).
9. `src/wm/interaction.rs:33-53` `PointerCommit::plan`: iterates the Vec by reference, stores `effects` by move. No clone, no alloc. `grab`/`ungrab` are scalar.
10. Consumed by `wm/effects.rs` `execute_effects` (not read; UNVERIFIED for per-effect String/format!).

### Allocation count per motion event (live, from the code above)

| Case | Heap allocs per motion |
| --- | --- |
| Move, `contents_while_dragging = false` (preview outline) | 1 (`:198`) |
| Move, contents drag, frame moved | 2 (`:213` vec of 2, then `:216` realloc to 4) |
| Move, no-op or clamp unchanged | 1 (`:188` or `:206`) |
| Resize, preview outline | 1 (`:262`) |
| Resize, contents drag, size changed | 2 (`:273` vec of 3, then `:277` realloc to 5) |
| Resize, no-op | 1 (`:244` or `:268`) |

Non-allocating: event pump collapse, `request`, `move_delta`, `move_anchor`, `PointerCommit::plan`, `step_owned`.

Not an issue on the live path: `reduce` (`reducer.rs:7-9`) clones the whole `ControllerState`, but only `interaction::step` (`wm/interaction.rs:79-83`, used by `wm/manage.rs:646` for Cancel) and tests call it. Motion uses `step_owned`.

Not read, so UNVERIFIED: `execute_effects` body (`wm/effects.rs` ~23-120, ~268-), `move_counters`, `flamewm_profiler::start`, `plan_move`/`plan_resize` internals under `frame/geometry`, and `wm/trace.rs` (may `format!` when tracing is off; not confirmed).

### Proposals (at most 2)

P1 (primary): replace the `Vec<FrameEffect>` return type of `reduce_owned`, `move_step`, `resize_step`, `configure_effects`, `move_effects`, `full_configure` with an inline small buffer, `type EffectPlan = smallvec::SmallVec<[FrameEffect; 8]>` (or an equivalent local `ArrayVec` if `smallvec` is not already a dependency; check Cargo.lock first). Max effects per motion is 5 (resize contents drag) and 6 (full_configure with release tail: 3 + 3 + Ungrab + DragOutline = 8), so `N = 8` keeps every motion plan inline. This removes the 1-2 heap allocs per motion in the table above. `PointerCommit.effects` becomes `EffectPlan`. `FrameEffect` must stay `Copy`/small; confirm `size_of::<FrameEffect>()` before choosing N.
   Regression check to add (unit test, host): assert a 10k-motion loop over `move_step`/`resize_step` with a preallocated `EffectPlan` yields zero heap growth (counting allocator in test only), or simpler: `assert!(!plan.spilled())` for the max-size plan.

P2: avoid the 1-alloc `vec![Noop]` returns entirely by returning a `FrameEffects` enum (`Noop | One(FrameEffect) | Many(EffectPlan)`), or, cheaper, make `PointerCommit::plan` skip storing Noop-only plans. Only valid if `execute_effects` treats `[Noop]` as empty. This is a second, independent change and is lower value than P1 because P1 already covers Noop inline.

### Next units
- Read `wm/effects.rs` `execute_effects` and `wm/trace.rs` for per-motion `format!`/`String` (the table above excludes them).
- Confirm `flamewm_profiler::start` and `move_counters` are allocation-free when disabled.
- Measure after P1 with the scoped perf test (one component, one second of simulated motion): count `wm.move.plan_or_reduce` share. Must be run by a builder; not run here.
