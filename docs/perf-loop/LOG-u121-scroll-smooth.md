# u121 scroll smooth

Area: Scroll feel in the terminal scrollback (explorer already had an event-driven sub-row glide and is unchanged).

Composition before: one wheel notch over the terminal scrollback moved `view_offset` by 3 lines instantly, and
`RunCache::update` treated any `GridOptions::view_offset` change as a key mismatch, so one notch rebuilt every
visible row: per-cell colour resolve, `glyph_rects`, run merge and one `String`/`Arc` per run, ~rows times. The
terminal also had no animation at all, so the whole jump landed in one frame and there were no glide wakeups to
remove.

Composition after: a 3-line notch is a ~4-frame ease-out glide. Each frame changes the visible window by one
absolute line, so `RunCache` rotates the existing row handles by that delta and rebuilds only the one exposed row
(plus the cursor row when it is on screen). One notch costs ~4 row rebuilds instead of one `rows`-row rebuild. The
ticker is demand-driven: one thread sleeps 16 ms only after a requested frame, sends one `Tick`, and parks on
`recv` again; at rest the terminal has the same zero idle wakeups as before.

Changes (crates/flamewm-terminal/src only):

1. `screen.rs`: `scrollback_drops: u64` increments at both capped `scrollback.pop_front()` sites and is exposed by
   `scrollback_drops()`. A drop shifts every absolute line index down without changing `total_lines`, so row reuse
   across a drop would show the wrong line.
2. `view.rs` `RunCache::update`: `drops` remembers the last counter. When the only key difference is `view_offset`
   (same palette and cursor shape, same row count, no dirty-all frame, same drops), the cache rotates its row
   handles by the absolute `top` delta (`rotate_left` when the window moved down a line, `rotate_right` when it
   moved up), marks only the newly exposed rows stale and rebuilds those. Any other key change, a dirty-all frame, a
   window move of at least `rows`, or a scrollback drop keeps the previous full rebuild. The capture-scroll branch
   is unchanged.
3. `scroll.rs` (new, `lib.rs` module): allocation-free `ScrollGlide` (`Clone, Copy`, no `Vec`/`String`) with
   `begin`, `is_animating`, `is_active`, `applied`, `pane`, `target`, `value`, `advance`, `cancel`. Ease-out-cubic
   over 70 ms; `advance` rounds, clamps inside the endpoints so it never overshoots, settles exactly on the target
   and clears its start.
4. `app/scroll.rs` (new): `spawn_frame_ticker` mirrors the explorer pattern (named thread, 16 ms only per
   requested frame, one `SessionEvent::Tick` plus one wake byte, exits when the sender drops, no panic if thread
   spawn fails). `schedule_scroll_tick` lazily creates the ticker and queues at most one in-flight frame;
   `advance_scroll` reads the glide copy first, cancels when the pane is gone, its offset no longer equals the
   glide's applied offset (Home/search/selection/tab external write) or the target exceeds scrollback, steps one
   frame, snaps exactly at the end, and snaps without a ticker.
5. `app.rs`: `scroll`, `scroll_timer`, `scroll_ticking` fields; `SessionEvent::Tick => {}` (no tab closes).
6. `ui.rs`: `project` calls `advance_scroll()` before building the frame; this is the per-Tick driver.
7. `app/pointer.rs`: the scrollback wheel branch computes the clamped target and starts a glide instead of
   assigning `view_offset`; mouse-report and alt-screen arrow paths stay instant. Other `view_offset` writers are
   unchanged: their external write is noticed by the applied-offset check and cancels the glide.

Expected composition shift: wheel scrollback shaping drops from ~rows run rebuilds per notch to ~1 per glide
frame (and ~0 per intermediate line while a line is reused), while no frame is scheduled after the glide settles.

Evidence (no cargo in worker): `scroll.rs` tests `a_glide_is_monotonic_and_settles_exactly_on_the_target`,
`a_reverse_glide_is_monotonic_downwards`, `an_equal_target_neither_moves_nor_panics`; `view.rs` tests
`a_view_offset_change_reuses_the_rows_between_the_windows` (rebuilt == 1, `Arc::ptr_eq` of old row 1 into new row
0 and the reverse step) and `a_scrollback_drop_forces_a_full_rebuild` (rebuilt == rows after a capped
`pop_front`). The existing `cached_runs_match_a_full_rebuild_over_random_updates` still compares the cache with a
full `grid_runs` rebuild over 3000 random edits, scrolls, selections, resizes, alt-screen switches and scrollback
overflow.

Verification: no cargo/git run (shared checkout); changes verified by reading the affected call paths.

Diff: crates/flamewm-terminal/src/{screen.rs,view.rs,scroll.rs,app.rs,app/scroll.rs,app/pointer.rs,session.rs,ui.rs,lib.rs},
this log, unit marked DONE.
