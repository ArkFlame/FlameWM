# u45-terminal3 log

Batch six over the terminal crate. `git` and `cargo` are forbidden in this shared checkout, so the review of the
previous slices is source-based (the batch logs of u04/u05/u20/u27/u34 plus the current code), and no build was run
here: the orchestrator builds, tests and commits.

## Review of the landed slices

Checked the batching read loop and pacer, dirty-row tracking, `RunCache` row reuse, in-place scroll rotation, pooled
parser params, the printer ground-state fast path, the packed scrollback forms, the ghost/at-prompt ioctl cache and the
in-place composed-row reuse. No functional regression found. One defect fixed: `main.rs` had a dangling doc comment
("Packaged asset root for the default wallpaper…", left over from a removed helper) attached to the
`settings_apply_tests` module; it is gone. The banner plumbing is still an intentional empty feature toggle
(`BANNER = []`, referenced by tests), so it stays.

## Next worst hotspots and what changed

- **Find recompute over the scrollback (`search.rs`, `screen/history.rs`, `screen.rs`).** `find` built
  `Vec<Vec<char>>` with one char per cell for every history line on every query edit: tens of thousands of heap
  blocks, several megabytes of transient text, plus a full `Cell` decode and allocation per packed line. It now
  walks the lines through a new `Screen::line_abs_into`, which decodes into one reused `Line` buffer via
  `History::cells_into` (`cells()` is implemented on top of it), and folds into one reused `char` buffer that `scan`
  matches in place. A find keystroke now allocates only the match list; both buffers grow once and are reused.
  `find_in` keeps its signature and reuses its fold buffer too. Expected shift: find typing cost drops from
  O(scrollback text + allocations) to O(scrollback cells) with no per-line allocator traffic.
- **Find repaint (`app/overlay.rs`).** `draw_find` scanned every match on every frame and built a throwaway
  `Vec<char>` per visible hit. Matches are in reading order, so the paint now binary-searches the visible line window
  (`partition_point`) and stops at its end, and builds each highlight string directly from the matched cells. A
  scrollback full of hits no longer costs a full-list scan per frame, only the visible hits.
- **Per-frame grid composition (`app/grid.rs`, `app.rs`).** `grid_frame` allocated `vec![Vec::new(); rows]` plus the
  per-pane pushes, the composed `Vec<GridRow>` and the output `Vec<Row>` every frame. The per-row input lists and the
  composed rows now live in `App` (`grid_inputs`, existing `grid_composed`), are cleared/truncated in place and are
  swapped with the entries so both sides keep their capacity. An idle, scrolling or flooding repaint allocates none
  of the ~40 per-row lists and none of the composed container; unchanged rows still keep their shared `Arc<[Run]>`.
- **Settings hover repaint (`app/pointer.rs`).** A hover/motion over the settings view returned `true` even when
  `hover_theme` reported `Command::None` (the pointer sat on the already-previewed row), forcing a full frame per
  pointer move. It now reports a change only when the command is not `None`. Also, a press on the already-focused
  pane no longer marks the frame changed by itself; the press path decides (selection, drag), so a click that changes
  nothing does not project.
- **Overlay row copies (`app/grid.rs`, `app/overlay.rs`).** `overlay` took `Vec<Run>` by value, so the ghost, find and
  menu paths copied the whole row (every run's `String`) via `to_vec()` and then copied the kept runs again. It now
  borrows `&[Run]` and only the runs that survive into the result are cloned. Halves the text copying for every
  overlaid row per frame while the suggestion, find bar or menu is open.
- **Selection text (`app/selection.rs`).** `copy_selection_in` built a `String` per selected line, joined them into
  another `String`, then cloned the pane's copy back to the caller. It now appends into `pane.selection_text` in one
  pass (trimming only the current line's tail so blank interior lines keep their newline) and returns whether there is
  a selection; callers borrow the pane text for the clipboard. One allocation instead of lines + join + clone, and no
  clone on every copy.

## Dead code / unused warnings

The brief lists unused imports and dead functions reported by cargo in `flamewm-desktop`, `flamewm-explorer` and
`flamewm-settings`. This unit owns `crates/flamewm-terminal/src/**` only, so those other crates were not touched; no
new dead code was introduced here (`line_chars` is still used by the link lookup and its tests, `take_dirty` by the
screen tests, and the new `line_abs_into`/`cells_into` are used by `find`).

Verification: no cargo/git run (shared checkout).
