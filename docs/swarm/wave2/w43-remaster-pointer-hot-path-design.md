# w43 — remaster the pointer motion hot path: design

Status: DESIGN complete; STEP 1 implemented in `frame/controller/planning.rs`, UNBUILT.
No `cargo`, build, test, git or python command was run by this unit. No new dependency added.
Visuals are unchanged: every effect keeps its value and order, so pixels stay identical.

## Reference-first evidence (read-only, `.vendor/**` is never a dependency)

| Reference path | Observed choice | Applied here |
| --- | --- | --- |
| `.vendor/smithay-master/src/input/dnd/mod.rs:7,41,48` | `SmallVec<[DndAction; 3]>` keeps a short event-owned action list inline | Same shape of problem (bounded effect list per event); inline buffering is the target, not a transplant |
| `.vendor/kwin-master/src/kscreenintegration.cpp:42` | `reserve()` before filling a `QVector` so a known-size batch allocates once | Reserve/reuse capacity instead of `vec!` per event |
| `.vendor/xfwm4-master/src/screen.c:728-750` | monitor index `GArray` allocated once and appended across updates | Caller-owned buffer survives across events |

`smallvec` 1.16.0 is already in `Cargo.lock:2757` but is not a direct dependency of
`flamewm-wm-x11` (`crates/flamewm-wm-x11/Cargo.toml:9-32`). Step 1 uses `std::Vec` only,
so no manifest or lock change is needed. A later step may justify promoting the buffer
to an inline type; that justification is recorded in step 2 below.

## Current structure (authoritative source, read this unit)

X11 motion path:

1. `src/event_pump.rs:60` -> `collapse_motions_result` (`:86-121`): no heap allocation (p96 row 1).
2. `src/wm/effects.rs:503` `interaction::step_owned`; profiler guard at `:499`.
3. `src/wm/interaction.rs:87-91` `step_owned` -> `reduce_owned` -> `PointerCommit::plan`.
4. `src/frame/controller/reducer.rs:14-17` `reduce_owned`; `FrameEvent::Motion` at `:112-118`
   dispatches to `move_step` or `resize_step` without cloning state.
5. `src/frame/controller/planning.rs`:
   - `:35-52` `configure_effects`: `vec!` of 3 (one allocation).
   - `:57-65` `full_configure`: buffer of 3 then `extend` of 3 (allocation + reallocation).
   - `:70-78` `move_effects`: `vec!` of 2 (one allocation).
   - `:81-99` `release_effects` / `release_tail`: `insert(0, ..)` on release only.
   - `:145-219` `move_step`, `:221-281` `resize_step`: every early return builds a new `Vec`.
6. `src/wm/interaction.rs:19-29` `PointerCommit.effects: Vec<FrameEffect>`; `:31-53` `plan`
   iterates by reference and moves the `Vec` (no clone, no extra allocation).
7. `src/wm/effects.rs:69-98` pacing filter collects a fresh `Vec` when a live resize batch
   is skipped; `:99-127` iterates `commit.effects`; `src/wm/mod.rs:562` stores
   `resize_deferred: Option<(Window, Vec<FrameEffect>)>`.

Wayland motion path:

- `crates/flamewm-wm-wayland/src/input/pointer.rs:122-150` normalizes/clamps the point;
  `output_layout.rs:134` `pointer_layout` returns a borrowed slice, so the clamp is allocation-free.
- `input/pointer.rs:151-258` resolves move/resize grabs; `:181-190` snap zone lookup,
  `:209-214` cross-output home refresh (gated on a boundary crossing, p94 row 4).
- `input/pointer.rs:259-265` resolves the target; a drag target is resolved directly to
  avoid the full scene walk.

## Problems counted, not measured

Counts come from `docs/swarm/pool/p96-reduce-alloc-audit.md:33-45` (static count of heap
sites on the live path). No allocator counter or profiler was run here, so these are
counts, not performance measurements.

| Case | Heap allocations per motion (p96) |
| --- | --- |
| Move, outline preview | 1 (`planning.rs:198`) |
| Move, contents drag, frame moved | 2 (`planning.rs:213` vec of 2, `:216` realloc to 4) |
| Move, no-op / clamp unchanged | 1 (`planning.rs:188` or `:206`) |
| Resize, outline preview | 1 (`planning.rs:262`) |
| Resize, contents drag, size changed | 2 (`planning.rs:273` vec of 3, `:277` realloc to 5) |
| Resize, no-op | 1 (`planning.rs:244` or `:268`) |

Secondary sites: `planning.rs:35-52` (three-element `vec!`), `:57-65` (grow 3 -> 6),
`effects.rs:78-94` (fresh deferred `Vec` per paced batch). p94 rows 4 and 5 record
unpaced, possibly allocating work in the Wayland snap/home-output lookups; those remain
UNVERIFIED for allocation count.

## Target design

One effect plan per input event, built into a buffer the caller owns across events:

- `configure_effects_into(&mut Vec<FrameEffect>, frame, extents)` clears and refills a
  caller-owned buffer. The old `configure_effects` stays as the byte-for-byte oracle.
- The `Wm` (X11) owns one plan buffer for the motion path and passes it by `&mut` through
  `step_owned` -> `reduce_owned` -> `move_step`/`resize_step`. All early returns write
  into the same buffer; no plan allocates after warm-up.
- `PointerCommit::plan_into(&mut Vec<FrameEffect>, ...)` derives grab/ungrab flags from
  the borrowed plan, with an owned constructor kept for non-hot callers.
- The pacing/deferral filter writes into a second caller-owned buffer instead of
  collecting a new `Vec` per skipped batch.
- Wayland: keep the current state updates; cache the home output per window and reuse
  the snap zone while the pointer stays in the same band (p94 rows 4 and 5), then
  coalesce to one planned effect per frame. Input geometry and focus authority are
  unchanged.
- Invariants for every step: effect values and order unchanged, `[Noop]` semantics
  unchanged, grab/ungrab ordering unchanged, and no native call moves earlier or later
  relative to the plan. These preserve pixels and WM authority.

## Steps (each step compiles on its own)

1. **This unit.** Add `configure_effects_into` in `planning.rs`, keep `configure_effects`
   unchanged as the oracle, build `full_configure` in the caller-style buffer, and add an
   equivalence test over several rects. No signature change outside `planning.rs`.
   Rollback: revert `planning.rs` only.
2. Add a crate-local `EffectPlan` alias and justify `smallvec` promotion if inline
   capacity is wanted (`smallvec` is already in `Cargo.lock:2757`; max plan length is 8
   per p96 row 52, so `[FrameEffect; 8]` fits inline). Keep the `Vec` buffer ABI so
   step 1 stays valid. Rollback: drop the alias and manifest entry.
3. Thread `&mut EffectPlan` through `move_step` and `resize_step`, converting each
   early return to clear-and-fill. Callers that do not own a buffer keep the existing
   functions. Rollback: restore the `Vec` returns in `planning.rs` and `reducer.rs`.
4. Add `step_owned_into` / `PointerCommit::plan_into` in `interaction.rs` and a plan
   buffer field on `Wm`; switch `effects.rs:503` to the `_into` variant. Rollback:
   restore `step_owned` at that call site; the new functions can remain unused.
5. Reuse the deferred-resize buffer in `effects.rs:78-94` and `wm/mod.rs:562`.
   Rollback: restore `.collect()` and the `Vec` field.
6. Wayland: home-output cache and snap-zone reuse in `input/pointer.rs`; then coalesce
   one planned effect per frame. Rollback: remove the cache fields and restore the
   lookups; no protocol-visible state changes.

Each step is independently buildable: none changes a public type until step 4, and
steps 2-6 keep a compiling fallback path.

## Proof required before any performance claim

After step 4, run one scoped perf test (one component, one second of simulated motion)
and compare the `wm.move.plan_or_reduce` share against a baseline, per
`flamewm-performance-proof`. Allocation elimination is a code-shape fact; a speedup is
not claimed here. Step 1 has no runtime evidence and is UNBUILT.

## Rollback summary

Step 1 touches only `crates/flamewm-wm-x11/src/frame/controller/planning.rs`; restoring
that file reverts this unit completely. The design document is documentation only and
can be deleted without affecting the build.
