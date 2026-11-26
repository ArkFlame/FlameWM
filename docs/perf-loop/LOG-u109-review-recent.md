# u109-review-recent — regression review of the recent perf slices batch seventeen

## Method

`git` (and `cargo`) are forbidden in this shared checkout, so `git log -40 -p on crates` could not be
run. The review used the evidence that is available offline: the 40 most recent unit logs
(`docs/perf-loop/LOG-u69-*.md` … `LOG-u108-*.md`, the perf-loop commit corpus, newest first) and then
the current sources each of them describes, read line by line against the four regression classes in
the brief: dropped wakeups, missed redraws after a state change, races on the new worker threads, and
lost cancel paths. Where a claim depended on a dependency's behaviour, the dependency source was read
(`~/.cargo/registry/src/.../smithay-0.7.0`, the exact version in `Cargo.lock`), not the vendored
master copy.

All statements below are source reads. No build, no runtime sample: composition claims are reasoning
about call paths, per `.agents/GOAL-PERF-LOOP.md`.

## Two defects found and fixed

### 1. Missed redraw: a parent commit that only moves a subsurface (u95, `handlers.rs`)

`CompositorHandler::commit` (`crates/flamewm-wm-wayland/src/handlers.rs:181-185`) requests a paint
only when `commit_changes_pixels` says the commit carries pixels. That predicate
(`handlers.rs:226`) tested only `SurfaceAttributes`:

```rust
attrs.buffer.is_some() || !attrs.damage.is_empty() || !attrs.frame_callbacks.is_empty()
```

u95's rationale was "a commit that carries no new buffer, no damage and no frame request changes
nothing on screen". That premise is false for a **parent surface with subsurfaces**. smithay 0.7
`impl Cacheable for SurfaceAttributes` (`wayland/compositor/handlers.rs:111`) *takes* the buffer,
damage and frame callbacks out of the pending state when building the committed snapshot, so during
the commit handler those three are exactly "what this commit attached/damaged/called back". A
subsurface's placement is not part of `SurfaceAttributes` at all: for a synchronized subsurface
(the protocol default, `SubsurfaceState.sync == true`) `wl_subsurface.set_position` is applied by
`PrivateSurfaceData::commit_sync_surface_tree` **on the parent's commit**
(`wayland/compositor/tree.rs:261-282`), i.e. the parent's commit is the one moment the moved child
becomes visible — and that commit has none of the three attributes, so it was skipped and the child
stayed at its old place until unrelated damage arrived.

Fix (`handlers.rs:233-244`): when the traversal reaches any surface other than the commit's own root,
the tree has a child, so treat the commit as pixel-changing. The self entry that smithay keeps at the
front of `children` (`tree.rs:122`, `children.push(surface.clone())`) compares equal to `surface`, so
leaf surfaces — layer panels, X11 toplevels, every surface without subsurfaces — keep the
damage-only gate and u95's idle-wake win. No new imports: the change is one comparison on the
`WlSurface` already in the signature, and `handlers.rs` was verified formatting-clean with
`rustfmt --edition 2024 --check` (exit 0).

Residual (documented, not fixed): a *desynchronized* subsurface applies its own `set_position` on its
own commit, where the tree root has no children; that commit is still skipped unless it carries a
buffer. Covering it needs per-surface last-committed-placement shadow state, which is a larger change
than this review unit should make without a build. `set_buffer_scale`/`set_buffer_transform`-only
commits are in the same class: smithay carries those two fields forward in the pending state, so a
commit handler cannot see that they changed.

### 2. Missed overwrite prompt: the conflict probe trusted a streaming listing (u104)

`crates/flamewm-explorer/src/app/transfers.rs` (u104) answers "does the target name exist?" from the
rows on screen for a paste into the current folder, instead of one `symlink_metadata` per source.
The condition was only `directory.as_path() == current.path()`. A tab whose listing is still
`loading` paints streamed batches (`Explorer::append_scan`, `lib.rs:432`), so its rows are a prefix of
the directory. A name that exists but is not in the streamed prefix at that moment counts as
"no conflict", the batch starts with `Conflict::KeepBoth`, and the overwrite question is never asked
(no data loss — the worker keeps both — but the prompt and the "Replace?" path are skipped).

Fix (`transfers.rs:151`): take the row shortcut only when `!current.loading`; otherwise fall back to
the previous per-source `symlink_metadata` probe. The fast path is unchanged in the settled listing,
which is the state a paste is normally issued from. Verified parse-valid with
`rustfmt --edition 2024 --check` (formatting diffs only: the file is intentionally dense style).

## Checked and sound (no change needed)

- **u101 search wake coalescing** (`search.rs:336-447`): `wake_once` sends the event before flipping
  `wake_pending`, and `poll` clears the flag *before* `try_iter`, so a batch is either drained by the
  poll that cleared the flag or queues its own wake. No lost wake, no double-drain. The stale-filter
  `id == self.next_id` is correct because `start` assigns `id = next_id` after `cancel()` bumped it
  and never increments afterwards, so the live job's id equals `next_id` until the next cancel.
- **u101 `search_running`** (`app/navigation.rs:23-47,49-69`): cleared on `Done` and `clear_search`;
  a cancellable `Done{cancelled}` is always stale-filtered out, so the `continue` cannot leave a
  stale "Searching…". A repeated Enter keeps the live walk; a changed query/folder still cancels.
- **u96 thumbnail cancel path** (`thumbs.rs:464-580,608-671`, `workers.rs:57-75`): an abandoned
  in-flight decode clears `in_flight` and the caller drops its `pending` mark, so the row can be
  requested again; `wants` re-checks after the header probe and before the whole-file read, and a
  cancelled decode sends no result and no wake byte. A scroll-back that lands before the abandoned
  decode notices can decode the same file one extra time (the old decode then completes and caches,
  the requeued job decodes again); that is one redundant decode, never a wrong or missing thumbnail.
- **u97 pager publish / clock tick** (`app/panels.rs:230-238`, `app/timers.rs:126-177`): the publish
  flag is retried on any later projection (`!published || changed.panels || changed.workspaces`), so
  a panel that was not laid out yet cannot stay unpublished; the clock tick still writes the date
  visibility on midnight and on a forced tick (`time_zone_changed`, settle, settings revision), and
  `clock_projected_sec` still guards the once-per-second write.
- **u107 startup settings workers** (explorer `main.rs:397-408` + wake at 507-554; task-manager
  `lib.rs:1124-1131` + wake at 1258-1284; shell `main.rs:745-758` + fd source at 907-926): each
  sends the snapshot into a one-slot channel *before* writing the wake byte, and the byte is buffered
  by the socket if the loop is not yet parked on it, so the snapshot cannot be lost. No shared state
  is touched off the UI thread; the theme and backdrop are applied on the wake turn, which also
  projects/paints in the same turn.
- **u86 occlusion culling / u94 redraw scheduling**: a culled window leaves the element list, and the
  damage tracker damages the previous geometry of vanished elements, so the move-away case repaints;
  opacity is only taken from the window's own opaque regions, and opaque memory chrome is excluded
  from occluders. The two u94 pacing fixes (frame request during a paint, stale pacing clock) close
  the only dropped-request paths found on that route.
- **u82 switcher change detection** (`switcher.rs:53-68,125-132`): `same_content` compares `open`,
  `immediate`, `selected` and the whole `entries` list (full `PartialEq`, so every label/metadata
  field), plus preview count, size, generation and (legacy) pixel identity. No visible field is left
  out of the dirty check.
- **u108 id-buffer reuse** (`projection/panel.rs:383-403,438-457,750-810`,
  `async_projection/task_slots.rs:167-191`): every reused `String` is `clear()`ed before its `write!`,
  the progress id is built by the caller in its own buffer, and `project_task_progress` is the single
  writer of `task-slot-<n>-progress`; the static battery-menu glyph table is byte-identical to the
  template ids (checked against `ui/shell/battery.html`).
- **u104 batched progress** (`flamewm-archiver/src/progress.rs:124-159`): a gated step keeps
  `pending_bytes`/`pending_entries`, completion (`bytes_done >= total` and `entries_done >= total`)
  bypasses the 100 ms interval, and the archiver window drops the progress model on `Reply::Done`
  (`archiver-app/src/window.rs:439-458`), so a deferred intermediate snapshot cannot strand the bar.

## Notes for the next reviewer

- The u108 log cites `crates/flamewm-shell/src/projection/async_projection/task_slots.rs`; the file is
  `crates/flamewm-shell/src/async_projection/task_slots.rs` (no `projection/` segment).
- Recommended follow-up unit (needs a build): store the last rendered state per surface
  (`buffer_scale`, `buffer_transform`, `SubsurfaceCachedState::location`) in `SurfaceData.data_map` and
  compare it in the commit handler so the two residual cases above can also keep the damage-only gate
  without risking a missed redraw.

## Files changed

- `crates/flamewm-wm-wayland/src/handlers.rs` — parent-with-children conservatism in
  `commit_changes_pixels`.
- `crates/flamewm-explorer/src/app/transfers.rs` — row-based conflict probe only for a complete
  listing.
- this log; unit marked DONE.

## Composition shift

Both fixes restore work only on paths that were missing it: a parent commit with a subsurface now
paints when it can have moved pixels (a commit rate well below the panel damage rate, and only for
clients that use subsurfaces — layer panels and X11 windows are unaffected), and the explorer asks the
kernel about target names only while its listing is incomplete (a few ms after a navigation or watcher
event). The steady-state shares of u95/u104 are not given back.
