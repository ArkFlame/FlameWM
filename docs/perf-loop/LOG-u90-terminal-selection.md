# u90-terminal-selection log

Batch thirteen over the terminal crate's selection, scroll and resize paths. `git` and `cargo` are
forbidden in this shared checkout, so this slice is source-read and compile-checked by inspection only:
the orchestrator builds, tests and commits.

## Review of the landed slices

Re-read the packed scrollback forms (`screen/history.rs`), the reused decode buffers in
`Screen::line_abs_into` / `RunCache` row rebuilds, the in-place scroll rotation, the per-pane `RunCache`
row reuse, the drag path that stores the span without copying, and the single-pass `copy_selection_in`.
No regression found; the remaining work is per-event resize cost.

## Changes

- **Resize, alternate grid (`screen.rs`, `Screen::resize`).** Every ConfigureNotify resized the lines of
  *both* grids, so an inactive alternate grid paid `rows × cols` of `Vec::resize` padding/truncation for
  pixels nobody draws. `switch_alt` already clears and sizes the alternate grid on activation, so the
  width pass now runs only for the primary grid, plus the alternate grid while it is active. A
  height-only resize now touches no line buffer at all.
- **Resize, damage bitmap (`screen.rs`).** The per-row dirty bitmap was reallocated on every resize; it is
  now cleared and resized in place, keeping its capacity across a ConfigureNotify storm. `mark_all` still
  forces the one needed full frame.
- **Resize, tab stops (`screen.rs`).** Tab stops depend on the column count alone, yet every resize rebuilt
  the `Vec<bool>` and discarded stops the program had set with HTS. The list is now rebuilt only when the
  column count changes, which both removes an allocation per vertical resize and preserves the stop set.
- **Resize, history pull/push (`screen.rs`).** Shrinking the window collected the departing top lines into
  a temporary `Vec<Line>` before packing them into scrollback; the drain now packs each line as it leaves,
  borrowing only the primary grid, so the batch needs no extra container. Growing pre-sizes the pull list
  to the row delta instead of growing it a reallocation at a time.
- **Selection copy (`app/selection.rs`).** `copy_selection_in` decoded every selected line through
  `line_abs_into`, which copies a visible row into the scratch `Line` first. Visible rows are now borrowed
  from the grid via `Screen::live_line`; only scrollback lines are decoded into the one reused buffer.

## Expected composition shift

Window resizing is the hot slice here: a drag now costs one grid pass instead of two, none of it on line
buffers when only the height moves, no bitmap allocation, and no tab-list rebuild. Selection copy over the
visible screen drops its per-row cell copy while long scrollback copies keep the single decode buffer.
Scroll and selection rendering are unchanged (dirty-row rebuilds, `Arc<[Run]>` reuse, no per-motion copy).

## Bounded scrollback

Still count-bounded (`scrollback_limit` lines), with each stored line bounded by its trimmed text plus
style runs (or one full row when style churn makes packing a loss), so memory stays `limit × O(cols)`. No
change needed here; the packing forms already keep ordinary output far below the `Cell`-per-column worst
case.

Verification: no cargo/git run (shared checkout).
