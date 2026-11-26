# LOG u109-review-recent — regression review of the last 40 perf commits

Batch seventeen. Owned paths: `crates/**/src`. No `cargo`, no `git` in the shared checkout, so the
reviewed diff is the unit LOG corpus for the 40 most recent commits (u69–u108, which the 40 newest
`docs/perf-loop/LOG-u*.md` files describe one-to-one) plus the current sources they changed. Every
statement below is a source read; the orchestrator builds and runs the tests.

## Method

The four regression classes in the brief were checked per unit against the current code, not against
the log's claim:

- dropped wakeups (worker → UI notification paths),
- missed redraws after state change (paint/invalidate predicates and restart paths),
- races on the new worker threads (send/flip/drain ordering, channel capacity, non-blocking write),
- lost cancel paths (in-flight cancellation, pending marks, stale-generation filters).

Focused units (a mechanism in one of the four classes): u80, u82, u83, u84, u85, u86, u87, u88, u89,
u91, u94, u95, u96, u97, u100, u101, u102, u103, u104, u106, u107, u108. The profiler/scenario units
(u92, u93, u99) and the deletion unit (u98) were checked only for runtime effect (none).

## Fixed

### F1 — u95 commit gate skipped placement-only commits (missed redraw)

`crates/flamewm-wm-wayland/src/handlers.rs::commit_changes_pixels` treated a commit as paintable only
when a surface in the tree carried a buffer attach/removal, damage, or a frame callback. Smithay 0.7
makes those three the *only* visible attributes: `Cacheable::commit` for `SurfaceAttributes`
(`wayland/compositor/handlers.rs:111`) takes `buffer`, `damage` and `frame_callbacks` out of the pending
state, so `current()` inside the commit handler reports exactly what this commit attached and damaged.
Placement is not in that set: `wl_subsurface.set_position` (and the whole synchronized child state) is
applied by the transaction on the **parent's** commit and lives in `SubsurfaceCachedState`, so a parent
commit that only repositions a subsurface carried none of the three and was dropped — the moved
subsurface stayed at its old pixels until some unrelated damage. Same gap for a surface/subsurface
`set_buffer_scale`/`set_buffer_transform` re-commit (`SurfaceAttributes::commit` copies the scale forward,
so it is not a change signal either).

Fix: a surface whose commit tree contains a real child (subsurface) is now treated as paint-worthy,
because child placement can change pixels with no buffer, damage or callback of its own. The
`with_surface_tree_upward` traversal visits the surface as its own first child (smithay
`PrivateSurfaceData::init` pushes the surface itself), so `child != surface` is false for the root and
true only for real subsurfaces; the visit is one pointer compare per surface and does not change the
damage-only gate for leaf surfaces (X11 windows, layer-surface panels, notifications — the traffic u95
targeted).

### F2 — u104 conflict probe trusted a partial listing (unprompted overwrite)

`crates/flamewm-explorer/src/app/transfers.rs::start_transfer_to` answers the overwrite question from
the on-screen rows instead of stat'ing each source. The rows are only authoritative once the listing is
complete: a tab that is `loading` shows streamed batches (`append_scan`), so a name missing from the
partial list can still exist on disk, the conflict count came out low, and the transfer started without
the conflicts dialog. The worker still re-checks targets and the default is `KeepBoth` (no data loss),
but the user lost the prompt. Fix: the listing shortcut now requires `!current.loading`; an incomplete
listing falls back to the existing per-source `symlink_metadata` path.

## Residual risks (verified, not changed — no safe edit without a build)

- **R1 (leaf-surface scale/transform).** With F1, a placement change on a surface that has no
  subsurfaces is still skipped when it changes only `set_buffer_scale`/`set_buffer_transform` and no
  buffer/damage/callback is attached. Both attributes are carried forward by `Cacheable::commit`, so the
  current commit cannot be distinguished from an unchanged one. Closing it needs a per-surface shadow of
  the last painted scale/transform; that is a larger change than this review unit's budget and needs a
  runtime check.
- **R2 (desynchronized subsurface `set_position`).** A desync child applies its position on its **own**
  commit, where it is the traversal root and has no children, so F1 does not cover it. Same shadow-state
  fix as R1.
- **R3 (u96 abandoned decode).** `retain` cancels the in-flight key and clears its `pending` mark so the
  row can be re-requested; if the row returns before the decoder reaches its next `wants` check, the key
  is queued again and the same file decodes twice. One extra decode in a rare race, no stale cache (the
  cancelled decode is not cached), so it was left as the cancellation contract describes.
- **R4 (u104 case-insensitive alias).** The row-name probe is exact-match; on a case-insensitive
  destination an existing `Foo` is not a conflict for a pasted `foo`. The worker re-checks and keeps
  both, so no data loss; a case-folded compare needs the destination filesystem's folding rule, which the
  explorer does not track.
- **Doc only (outside owned paths).** `LOG-u108-alloc-hot.md` names
  `projection/async_projection/task_slots.rs`; the file is `crates/flamewm-shell/src/async_projection/task_slots.rs`.

## Verified sound (the mechanism the unit changed holds)

- **u101 search coalescing — no lost wake.** The walker sends the batch/timeout event before
  `wake_pending.swap(true)` (`search.rs:361-392`) and `poll` clears the flag before draining
  (`search.rs:436-447`), so a batch is always in the channel when its wake is suppressed; a batch sent
  during the drain schedules a fresh wake. The stale-id filter is consistent with `start`, which sends
  `id = next_id` after `cancel()` bumped it, so a live job always matches `active`.
- **u96 cancellation — every abandon path returns the key.** `retain` returns queued jobs plus a
  dropped in-flight key; `start_pass_keeping` removes their `pending` marks and the worker's `wants`
  check aborts between header, read and decode stages. `Cancelled` never caches a failure.
- **u97 pager publish / clock tick — no missed projection.** `pager_cells_published` stays false until
  every panel accepts, and the partial tick retries while it is false
  (`app/panels.rs:230-238`). The clock defers the date string only when the local date did not change and
  the tick is not forced; a settings/time-zone/date change takes the full `project_clock_at`
  (`app/timers.rs:129-164`), which also re-applies the date visibility.
- **u97 status views — one pass, same rules.** `status::views` applies the audio/media/network rules to
  the base views and `tray_any_visible` reads those plus battery/weather/SNI, matching the popover
  projection's predicates.
- **u107 settings workers — send before wake, one-shot.** Explorer, task manager and shell all call
  `try_send`/`send` and write the wake byte only after the snapshot is queued; the shell's self-pipe
  drains then `try_recv`s in one reactor callback. A fresh socket pair cannot drop the single byte, and
  a failed fetch leaves the defaults the first frame already painted.
- **u86 occlusion culling — damaged by the tracker.** A culled window vanishes from the element list and
  `OutputDamageTracker` damages its previous geometry; only opaque client bodies are trusted as
  occluders, memory chrome never is.
- **u82 switcher no-repaint guard — complete key.** `SwitcherFrame::same_content` compares open,
  immediate, selection, every entry field (full `entries` equality) and every preview (size, WM
  generation, legacy pixel identity).
- **u108 id buffers — cleared per slot.** Both the sync panel pass and the async task-slot pass clear and
  re-`write!` `slot/icon/label/line/progress` ids for each slot; the pager layout/cell ids and the battery
  menu ids use one cleared buffer or a static table.
- **u104 progress gate — completion still reports.** The 100 ms gate keeps pending byte/entry counters,
  `complete` bypasses the interval, and the archiver window clears the bar on `Reply::Done`.

## Composition shift

- F1 costs one pointer compare per visited surface in the commit walk and restores paints only for
  commits of windows that own subsurfaces (video/embedded windows), which painted before u95; the
  damage-only skip survives for the dominant leaf traffic (X11 windows always attach a buffer; shell
  panels are layer surfaces and have no subsurfaces). No measurable change expected in the idle sample.
- F2 replaces a set lookup with per-source stats only while a tab is streaming (a few milliseconds after
  a navigation or watcher event), instead of answering from incomplete rows.

## Verification

- `rustfmt --edition 2024 --check crates/flamewm-wm-wayland/src/handlers.rs` → exit 0 (parses and is
  format-clean).
- `rustfmt --edition 2024 --check crates/flamewm-explorer/src/app/transfers.rs` → parses (no parser
  errors); the file is not rustfmt-normalised repo-wide, so the check reports the pre-existing
  differences, including the edited line.
- No `cargo`, no `git`. Compile and tests are the orchestrator's build.
