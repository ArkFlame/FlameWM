# LOG u67-taskmgr-tick — task manager 1 s tick (sampling + projection)

Area: `crates/flamewm-task-manager/src/**`. No `cargo`, no `git` in this unit (shared checkout —
the orchestrator builds, tests and commits).

## Review of the earlier task-manager slices (regressions)

Compared the current tree with the recorded intent of `LOG-u26-taskmgr-ui.md`,
`LOG-u33-taskmgr2.md`, `LOG-u44-taskmgr3.md` and `LOG-u55-taskmgr-clone.md`; no `git log` in this
checkout. Invariants re-checked by reading:

- hot-set replay: `cached.last` is still stored for every pid (hot included), so a pid leaving the
  hot set cannot replay a pre-hot sample.
- CPU delta: `known = cached.seen != 0 && cached.start == task.start`, span is
  `total - cached.read_total`; PID reuse still cannot inherit a counter.
- rotating schedule `slow_due(pid, tick)` and `hits`/`hits_dirty` caching are unchanged.
- `cached.last` clone + the snapshot copy are intentional (`Arc<str>` fields make them refcount
  bumps); no double-allocation regression found.

One piece of dead state found and removed: `Cached.uid` was written and then read once in the same
block, so it is now a local (`Cached` is 4 bytes smaller per tracked pid).

## Changes (composition-share ranked, visible-only work)

1. **Graph raster: fill/store instead of source-over per pixel** (`graph.rs`). Per sample the
   three cards walked a cleared raster and pushed every pixel through the full `blend` entry point:
   index math, `get_mut` range check, alpha test, and — for the translucent area — the f32
   source-over. The area under the curve is roughly half of a 208x88 raster per filled card, so it
   is the widest primitive the tick draws. Now `fill_below` walks one column with a fixed stride
   and does a plain 4-byte store for empty pixels (the raster is cleared first, so only the grid
   dots are non-empty) and `blend_pixel` only for those dots; the line pass uses `store`, a direct
   4-byte write, because the line is always opaque. Output is byte-identical: the opaque and
   empty-target cases of `blend` already resolved to a copy. Expected shift: removes the
   float source-over and the per-pixel bounds-checked call from the largest draw primitive of the
   render share.

2. **Core meters: emit a lane only when its rendered percent changes** (`lib.rs`). Every 1 s sample
   `paint_metrics` re-emitted 32 lanes: `core-col-*`/`core-*` visibility (36 document calls), the
   static `0`..`31` labels, and per lane `text_ref` + `size` + `background` (~96 document calls and
   32 colour interpolations). Lane visibility and labels depend only on the core count, so they are
   emitted when it changes; a lane whose integer percent text (the thing actually displayed) is
   unchanged now keeps its previous text, fill width and colour. Resizing sets `core_marks_dirty`
   because the track width follows the window. Expected shift: an idle desktop drops ~130 document
   calls and 32 `load_color` interpolations per tick from the projection share; a busy one only
   pays for the lanes that moved.

3. **Table: row visibility only when the visible row count changes** (`lib.rs`). The table restated
   `doc.visible` for all 40 row slots on every paint — sample, click, hover, scroll — while at the
   default window size only 15 can be visible. `painted_rows` now tracks the slots marked visible
   and only the boundary range is emitted (0..=delta calls). Expected shift: removes 25 always-empty
   document calls per paint from the projection share.

4. **`History` iteration without a modulo per element** (`graph.rs`). `in_order` used
   `(start + index) % SAMPLES` per value; it now chains the two slice halves and takes `len`. It
   runs for every drawn series (60 values x 3 cards) and in `peak()` for the network scale.

5. **Row name formatting without the `fmt` machinery**: `value.push_str(indent); value.push_str(..)`
   instead of `write!("{indent}{}")` for the one cell that carries a pre-indented label.

Composition note: the worker thread is dominated by procfs reads (unavoidable, already rotating +
hot-set scaled), the hot-set/depth work is O(pids), so the remaining discretionary share of the
1 s tick is the projection: three graph rasters plus the document calls. Items 1–3 attack that
slice.

## Evidence

`bench_graph_raster_per_sample` (graph.rs, `#[ignore]`, 300 simulated ticks = 5 minutes of the
1 s tick) reports `per_sample_ms` for the three cards alone, so the raster share can be compared
against `bench_sampler_cpu_per_sample` (model.rs) on the same machine.
`column_fill_stores_empty_pixels_and_blends_onto_existing_ones` pins the fill/store semantics
(straight store on empty pixels, source-over on a grid dot, overwrite on the line pass).

## Tests

- New graph test above; existing graph tests unchanged and still exact (they compare raw pixels).
- `cached.uid` removal needs no test change (`stat_rss_and_status_uid_match_the_legacy_parser`
  still covers `parse_uid` behaviour through `cached.user`).

No `cargo`/`git` executed (shared checkout); changes are compile-reviewed by reading.
Commit left to the orchestrator batch build.
