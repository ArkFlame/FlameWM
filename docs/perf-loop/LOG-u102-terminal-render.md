# u102 terminal render

Area: Terminal render pane (run shaping + frame projection).

Composition before: while output streams, every captured scroll marked the whole grid dirty, so each frame
re-shaped all visible rows (per-cell colour resolve, glyph_rects, run merge, one `String`/`Arc` per run) even
though the terminal window had only moved up by the scrolled lines. Colouring the pipeline bench, "shape: cold
RunCache" was the per-row cost paid ~rows times per scroll frame; the UI projection then wrote the shifted rows
anyway. When the window was fully obscured (minimised / covered), parsing kept running but the projector still
rebuilt the frame and wrote every node, work no pixel could show.

Changes (crates/flamewm-terminal/src only):

1. `screen.rs`: `scroll_up` records how many lines a capture scroll moved (`scroll_shift`), exposed through
   `take_scroll_shift`. `mark_all` (resize, alt-screen switch, scrollback erase) resets it, so the shift is only
   trusted for a pure scroll. `take_dirty_into` now also returns the per-row damage behind `dirty_all`, so rows
   edited in the same batch as a scroll are rebuilt at their new positions.
2. `view.rs` `RunCache::update`: on a capture scroll (dirty-all + trustworthy shift, absolute window advanced by
   exactly that many lines) it rotates the previous row handles (`Vec::rotate_left`, Arc moves, no run rebuild)
   and re-shapes only the rows the scroll exposed at the bottom. Rows edited, the cursor row, the row that
   previously painted the cursor and a changed selection are still rebuilt. A capped scrollback keeps the
   absolute index while the content moves (selection renumbers), so it falls back to a full rebuild.
3. `main.rs`: a visibility flag (from `UiControllerEvent::Visibility`) skips frame projection while the window is
   fully obscured; output still parses into the screen, and one projection runs when it is uncovered.

Expected composition shift: capture-scroll frames drop from ~rows run rebuilds to ~2 (exposed row + the reused
row that painted the cursor); a 60-row xterm pays 2/60 of the previous shaping work per scrolled line, and the
rest of the row handles are Arc clones. Obscured-window output drops projection to ~0 until uncover.

Evidence (no cargo in worker): `view.rs` tests `a_scroll_reuses_the_shifted_rows_and_rebuilds_the_exposed_bottom`
(rebuilt == 1, `Arc::ptr_eq` of old row 1/2 into new row 0/1), `a_scroll_clears_the_cursor_row_it_reused`
(rebuilt == 2), `a_resize_still_rebuilds_every_row`; the per-typed-char/scroll bench now asserts
`terminal_scroll_row_rebuilds == 2` instead of `rows`. `cached_runs_match_a_full_rebuild_over_random_updates`
still compares the cache against a full `grid_runs` rebuild over 3000 random edits, scrolls, selections, resizes,
alt-screen switches and scrollback overflow, which is the regression guard for the reuse.

Diff: crates/flamewm-terminal/src/{screen.rs,view.rs,main.rs}, this log, unit marked DONE.
