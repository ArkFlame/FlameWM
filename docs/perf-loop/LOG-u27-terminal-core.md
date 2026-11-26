# u27-terminal-core log

Area: terminal screen/pty/glyph rendering — scrollback memory, damage rects, parse-loop allocations. Third pass on the
terminal crate; earlier passes covered the batching read loop, dirty rows, resize and suggest/history work.

Changes (owned paths only):

- glyphs.rs `glyph_rects`: returns a fixed-capacity `Rects` value (four `Arm`s on the stack) instead of `Vec<(Arm, u32)>`,
  so every box-drawing, block and shade glyph in a rebuilt row stops allocating. Replaced the linear `BOX` table scan
  with a direct `match` (`box_weights`), O(1) per border cell on screens such as `htop` and curses UIs. Removed the
  table and the per-call closure. `view.rs` consumes `rects.iter()` and `extend_bands(&Rects)` unchanged in behaviour.
- screen/history.rs: new `History::Uniform { text, fg, bg, attrs }` for single-style scrollback lines, the dominant case
  for shell and build output. It drops the boxed run slice entirely: one less heap block (plus allocator metadata) per
  scrollback line and one less allocation on every scroll-out. `pack` now starts the run list small instead of
  reserving one run per column, cutting the large per-line scratch allocation; high style churn still falls back to
  plain cells exactly as before.
- parser.rs: pooled `;`-group vectors (`spare`, capped at 8). `reset_params` keeps the cleared group buffers for the
  next sequence and `separator` reuses one, so SGR-heavy output (`\x1b[1;31;4m`) no longer allocates a `Vec` per
  parameter per sequence. `digit` takes one `last_mut` borrow instead of two.
- view.rs `RunCache`: the per-frame `stale` bitmap moves into the cache and is reused (`take`/`resize`/`restore`), so a
  cache update allocates no scratch state. Dropped the `expect("checked above")` in `push_text` for an explicit match,
  removing a panic path.
- screen.rs damage rects: `scroll_up` marks `dirty_all` only when the scroll actually captures a line into scrollback
  (which shifts what a scrolled-back view sees); otherwise it marks just the scrolled row range. `scroll_down` always
  marks only the scrolled row range. IL/DL, reverse index and region scrolls now leave a fixed header or status line
  row clean instead of forcing a whole-grid rebuild. `erase_cells` uses a slice `fill`.
- session.rs `read_loop`: the next batch buffer is pre-sized to the previous batch length, so a flood allocates once per
  event instead of regrowing a fresh `Vec` through several reallocations, while prompt echoes stay small.

Expected composition shift: fewer allocator calls and less allocator metadata during scrolling and full-screen redraws
(scrollback -1 allocation per line; glyph rows -1 allocation per box glyph); less scrollback RAM per uniform line;
fewer per-frame allocations in the projection; smaller row damage for region scrolls; no parse-loop allocations for
multi-parameter SGR. Visuals and features are unchanged; every stored line form still round-trips cells exactly.

Removed: the `BOX` lookup table, the `one` closure, the `push_text` expect, and the full-grid dirty marking for
non-window-shifting scrolls.

Verification: cargo/git are not run by this worker (shared checkout); the orchestrator builds, tests and commits.
