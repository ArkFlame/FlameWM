# u54-terminal-clone log

Batch seven over the terminal crate. `git` and `cargo` are forbidden in this shared checkout, so the review of the
earlier slices is source-based (the batch logs of u04/u05/u20/u27/u34/u45 plus the current code), and no build was run
here: the orchestrator builds, tests and commits.

## Review of the landed slices

Checked the batched PTY read loop and pacer, dirty-row tracking and `take_dirty_into`, `RunCache` row reuse, in-place
scroll rotation, pooled parser parameters, the printer ground-state fast path, the packed scrollback forms, the
ghost/at-prompt ioctl cache, the in-place composed-row reuse, the find scan and highlight paths, and the selection copy.
No functional regression found. The one real defect in this area was allocation-shaped rather than behavioural, and is
fixed below (`clip` cloning a run's text only to drop it). No dead code was orphaned by the new slices: `row_runs` is
still the public `grid_runs` path, `Screen::line_abs` is still used by word selection, and `History::cells` by the resize
path.

## Next worst hotspots and what changed

- **Scrollback decode per rebuilt row (`view.rs`, `screen.rs`).** `RunCache::update` called `row_runs`, which went
  through `Screen::line_abs`; a packed/uniform history line decodes into a fresh `Line` (`Vec<Cell>`, one allocation per
  visible row per rebuilt row) every frame while a scrolled-back view or a flood is composed. The row builder is now
  `runs_for_line(line: &[Cell], …)`, and the cache holds one `Line` scratch that each history row decodes into
  (`Screen::line_abs_into`). A live row is passed over borrowed via the new `Screen::live_line`, so the common live
  flood keeps the zero-copy path. Expected shift: -1 allocation per rebuilt scrollback row per frame (a full
  scrollback frame of 50 rows no longer makes 50 heap blocks), and none for live rows.

- **Find highlight decode per visible hit (`app/overlay.rs`).** `draw_find` decoded every visible match's line with
  `screen.line_abs`, one allocation per highlighted row per frame while the find bar is open. It now borrows a live
  row through `live_line` and decodes a history row into one reused `Line` for the whole paint. Expected shift: the
  per-frame paint allocates no `Line` per hit; only the highlight runs it actually draws.

- **Selection copy decode per line (`app/selection.rs`).** `copy_selection_in` walked the selected range with
  `line_abs`, decoding a whole scrollback line per line copied, so copying a few thousand scrollback lines made a few
  thousand heap blocks before the text copy. It now decodes into one reused `Line` per copy. Expected shift: copying a
  long range allocates only the result string, not one `Line` per line.

- **Link lookup decode per pointer event (`app/pointer.rs`).** `link_at` decoded the hovered line with `line_abs` on
  every pointer move (an allocation per motion over a link). It now decodes into a stack-held local `Line` buffer.
  Expected shift: no allocation per hover/motion event.

- **`clip` cloning dropped text (`app/grid.rs`).** `clip` built a clipped run with `..run.clone()`, so it cloned the
  run's `String` text and immediately discarded it in favour of the newly clipped text: one wasted allocation and
  copy per clipped run (split panes, menu overlays, find highlights). The run is now built field by field with no
  clone. Expected shift: -1 `String` allocation per clipped run per frame.

- **Frame row container (`app/grid.rs`, `app.rs`, `ui.rs`).** `frame` allocated a fresh `Vec<Row>` after `grid_frame`
  collected a second one (`composed.iter().map(|e| e.row.clone()).collect()`), so every frame made two row vectors
  (one of them purely intermediate) plus the `vec![Row::from(bar)]` growth. `grid_frame` now appends straight into the
  caller's row list, and `App` keeps `frame_rows`, which the projector hands back after writing the document. Expected
  shift: an idle or flooding repaint allocates no row container and no intermediate vector once the window has been
  drawn at its size; the per-row handles are still only `Arc` bumps.

## Dead code

Nothing to delete in this pass: every helper either stays on a live path or is a documented public/test path
(`grid_runs`, `Screen::line_abs`, `History::cells`). No new dead code was introduced.

Verification: no cargo/git run (shared checkout); `rustfmt --check` parsed every edited file, so the changes are
syntactically valid. The orchestrator builds, tests and commits.
