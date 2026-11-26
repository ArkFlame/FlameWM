# u65-terminal-burst log

Batch nine over the terminal crate. `git` and `cargo` are forbidden in this shared checkout, so the review of earlier
slices is source-based (the batch logs of u04/u05/u20/u27/u34/u45/u54 plus the current code), and no build or test was
run here: the orchestrator builds, tests and commits. `rustfmt --check --edition 2024` parsed every edited file.

## Review of the landed slices

Checked the batched/paced PTY read loop, the `OUTPUT_BUDGET` drain-and-commit coalescer, the ground-state ASCII fast
path and pooled CSI parameters, `print_ascii`'s run write with unchanged-run detection, in-place scroll rotation and
dirty-row tracking, the three packed scrollback forms, `RunCache` row reuse with its reused dirty/decoded buffers, the
in-place grid composition and handed-back `frame_rows`, the pooled UI projection with per-row skip, and the cached
line modes for the prompt check. No functional regression found in those slices.

Two leftover defects were found and fixed rather than re-reported:

- `UiCache::empty` (documented as "the row handed to rows past the frame's end") was never used: `project_rows`
  allocated a fresh empty `Row` (`Arc` heap block) every frame. It now uses the kept row.
- `UiCache::ids` (documented as node ids formatted once when the pool changes) was written nowhere and read nowhere;
  each row still formats into its own local buffer. The dead field is deleted; `SlotState`'s documented behaviour was
  declared but also never implemented, and is implemented below.

The banner plumbing (`BANNER = []`, `banner_rows`, `update_banner`) is still an intentional empty feature toggle
referenced by the app tests, so it stays (same call as the u34/u45 batches).

## Composition and what changed

For one second of terminal output burst (`cat`, a build log, `seq`), ranked by share of the terminal's own CPU time on
the UI thread, ignoring X rendering and the child process:

1. **VT parse + cell apply (largest, per byte).** Unchanged this pass: it is already a two-scan run writer with the
   unchanged-cell skip; the remaining per-byte work is the intrinsic cell store.
2. **Scrollback packing on a full-screen scroll (second, per scrolled line).** `scroll_up` packs one line per scrolled
   row, and `History::pack` allocated a `String` **and** a `Vec<StyleRun>` (`Vec::with_capacity(...)`) for every line,
   then dropped the `Vec` immediately for the one-style case that ordinary output always takes. At flood speed that is
   two heap allocations per new line, on top of the text copy. `pack` now keeps the trailing run in registers and
   allocates the run list only when a second style actually appears, so a uniform line allocates only its boxed text
   (and no run list at all); `Packed` lines are unchanged.
3. **Retained-UI projection of changed runs (third, per changed run per frame).** `write_run` wrote position, size,
   foreground, background and font weight for every run whose text changed, although a counter tick, a log line
   scrolling in place or a same-width edit keeps the box and style. `SlotState` (already declared and documented for
   exactly this) is now filled per pooled slot on show and diffed on every write: only changed setters are written,
   and hiding a slot resets its state so showing it again writes every property. The `htop`/burst steady-state shows
   up to five fewer document setter calls per written run.
4. **Read path allocations (per output event).** The reader allocated a fresh `Vec<u8>` per batch (`with_capacity`) and
   the UI dropped it after parsing; at a 32 KiB batch that is a 32 KiB allocation and free per event. The UI now hands
   the consumed buffer back through a channel (`Session::recycle`) and the reader takes it for the next batch
   (`try_recv`, falling back to the old pre-sizing). The recycle cap is two batches, so memory stays bounded.
5. **Prompt-check ioctl (per output batch).** `Session::feed` ran `tcgetattr` on every batch, including flood batches,
   to refresh the cached line-discipline flags. Modes only change with a prompt or a key press, so batches over
   `PROMPT_SAMPLE_BYTES` (4 KiB) now skip the ioctl; the next small batch, and every key press, samples again. A flood
   no longer pays one syscall per 32 KiB.
6. **Per-frame tab-bar `Arc` (per frame).** `frame` rebuilt row 0 and wrapped it in a fresh `Arc<[Run]>` every frame
   (60/s in a burst) even when the tab bar was byte-identical. It now keeps the previous row 0 when its runs compare
   equal, which also lets the projector's `Arc::ptr_eq` fast path hit.

Expected composition shift for the burst sample: parse unchanged (still the top share); scrollback packing drops its
per-line `Vec` allocation and the uniform-line path loses one allocator round trip per new line; projection setter
traffic drops by roughly the share of runs whose geometry and style did not change; the read path moves from one
allocation per event to (after the first) none; the prompt ioctl leaves the flood path; the tab-bar `Arc` leaves every
frame. No drawn cell, colour, cursor, selection or scrollback content changes.

## Dead code

`UiCache::ids` deleted (never written, never read). `UiCache::empty` was dead and is now on the live path. No other
orphan found: `pending_dirty_rows` is the bench probe, `line_abs`/`cells` stay on selection and resize paths,
`take_dirty` on tests, and the banner toggle stays as documented above.

Verification: no cargo/git run (shared checkout). `rustfmt --check --edition 2024` on `ui.rs`, `session.rs`, `app.rs`,
`app/grid.rs` and `screen/history.rs` exits 0. The orchestrator builds, tests and commits.
