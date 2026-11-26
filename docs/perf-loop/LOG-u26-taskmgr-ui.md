# u26-taskmgr-ui — third pass log

Area: `crates/flamewm-task-manager` (sampler + table paint). Prior passes already made the
rotating `stat` schedule, the hot set, the static-field refresh, the visibility gate and the
visible-row-only table. This pass attacks the remaining per-sample allocation and redundant work.
No cargo/git was run; rustfmt (edition 2024, check mode) is clean.

## Change 1 — one procfs path buffer instead of one `format!` per task read

Before: `read_proc(&mut self.scratch, &format!("/proc/{pid}/stat"))` for every sampled pid every
tick, plus `status` and `cmdline` on static refresh. That is one `String` allocation, one growth
and one free per read (~200-400/s with ~400 tasks).

Now: `Sampler.path: String` with `proc_path(&mut path, pid, leaf)` writing in place. Same layout,
no per-read allocation. `cmdline` uses it too (`command` takes the buffer).

Composition shift: removes the sampler's dominant allocation churn; the remaining per-tick cost is
the syscalls that the data actually requires.

## Change 2 — reused pid list and a reserved process vector

`Sampler.pids: Vec<u32>` is cleared and refilled instead of collecting a fresh `Vec` from the task
map each second; discovery appends into the same buffer, then one `sort_unstable`/`dedup` pass.
`result.processes.reserve(self.pids.len())` removes the repeated grows of the snapshot vector.

Composition shift: the per-sample walk no longer allocates its working set.

## Change 3 — no replay clone for hot pids

`Sampler` keeps `Cached.last` so non-hot processes can replay their previous sample between
rotating reads. Hot pids are re-read every tick, so their `last` copy was written and never read:
`cached.last = (!hot).then(|| task.clone())` keeps the replay copy only where it is used.

Composition shift: up to `HOT_COUNT` fewer three-`String` clones per sample.

## Change 4 — allocation-free row cells (id + value buffers)

`paint_table` allocated a fresh id `String` per cell (`r-<slot>-<cell>`) and a fresh value
`String` per numeric/memory cell, ~200+ allocations per paint (the table repaints on every
sample and on every action/key).

Now one `id` and one `value` `String` are reused for the whole table (`cell_id`,
`memory_text_into`), and cells are written with `text_ref`. The document already compares before
dirtying, so unchanged cells (user, state, pid, threads, priority, nice, virt, cmd) now cost no
allocation at all; CPU and memory still reformat because they change.

`display_name` also returns `&str` instead of an owned `String`, removing one allocation per
visible row.

Composition shift: per-paint heap traffic in the table drops from ~200 allocations to the few
cells that actually change.

## Change 5 — menus only format when open

`paint_menus` used to look up the selected process and format the title plus five priority labels
on every paint even when both menus were closed. It now returns after the two `visible(false)`
calls when neither menu is open.

Composition shift: on a normal sample paint, six `format!` calls and a linear process scan
disappear.

## Change 6 — scrolling counts instead of sorting

Wheel scrolling needed only the filtered length; it called `sorted()` (full `O(n log n)` sort plus
a `Vec`) to get it. The shared filter is now `matches()`, and `matching_count()` returns the bound
without ordering. `sorted()` uses the same predicate, so the two cannot drift.

## Dead code / panic paths

- Removed `Process.uid`: it was written (`task.uid = cached.uid`) and never read; the user name is
  resolved from `Cached.uid`. One fewer field in every `Process` (snapshot, cache and clones).
- Reviewed the sampler and paint paths for reachable `unwrap`/index panics: the only `unwrap`s are
  in `#[cfg(test)]`; slice/index uses are bounded (`fields.len()` checks, `visible` guard before
  `hits[first + slot]`, `hits.len() - 1` behind a non-empty check, `INDENTS[depth.min(8)]`).
  No new panic path was introduced; the new buffers are locals and index-free.

## Next candidates (not in this pass)

- `paint_metrics` still `format!`s ~80 cells per sample (core labels/percentages, totals); the same
  reused-buffer + `text_ref` pattern applies.
- `assign_depths` rebuilds a `HashMap` of every pid each sample; the map could live in `Sampler`.
- Graph rasters are rebuilt per sample (73 KB each); the document already skips dirtying when the
  bytes are equal, so a buffer hand-off would need an API that accepts a shared buffer.
