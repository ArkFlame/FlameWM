# LOG u112-test-gaps — unit tests for batch 9-16 pure helpers

Batch seventeen, area: small host-only unit tests for pure helpers added in batches
9-16 that carried no test of their own. `cargo` and `git` are forbidden in this shared
checkout, so this slice is read- and edit-checked only; the orchestrator builds and runs
the tests.

## Composition reasoning

Tests do not change runtime composition; they lock in the composition wins of the
earlier units so a later refactor cannot silently reintroduce the work. The gaps were
ranked by the share the helper guards:

1. `Reporter` throttle (u104, archiver): guards the per-chunk sink/wake share of a copy
   or extract. Without the gate, one large file emits thousands of progress snapshots.
2. Grid row reuse (u102/u108, terminal): guards the per-frame compose share. A row is
   recomposed only when a pane row is a new allocation or moved; otherwise the shared
   `Arc` is handed out again.
3. Thumbnail LRU byte cap (u96, explorer): guards the decoder worker's memory and the
   per-scroll decode share. The existing test covered the entry-count cap and failure
   caching, not the pixel-byte budget.
4. CPU share math (task manager): already had one equality assertion via
   `sampling_never_counts_process_reuse_as_cpu_usage`; extended with the busy-delta
   share and the counter-regression case.

Share-math helpers elsewhere in batches 9-16 already carry tests (`cpu_percent` and
`percent` in the task manager and profiler, `scale_rates` in desktop widgets,
`row_values` in shell usage, `heat_alpha`, `progress_width_scales_percent_into_track`,
`task_slot_capacity_for_length`, `taskbar_css_width`), so no test was duplicated.

## Tests added

- `crates/flamewm-terminal/src/app/grid.rs` (`#[cfg(test)] mod tests`):
  - `same_inputs_compares_allocation_rectangle_and_pane_count` — pointer identity and
    rectangle are the reuse condition; a value-equal re-allocation recomposes.
  - `compose_row_hands_out_the_single_pane_allocation_when_it_fits` — the shared-row
    fast path returns the same `Arc`; an offset or clip builds a new row.
  - `compose_row_places_panes_side_by_side_and_drops_runs_past_their_edge` — pane
    offsetting, right-edge clipping and run dropping.
  - `overlay_replaces_only_the_middle_and_clips_the_edges` — left/right fragments keep
    their columns around the menu.
  - `clip_cuts_text_and_keeps_shapes_inside_the_new_run` — a block glyph's arm stays
    inside the cut run.
- `crates/flamewm-archiver/src/progress.rs` (`#[cfg(test)] mod tests`):
  - `reporter_opens_with_a_zero_snapshot_and_suppresses_sub_step_updates` — the
    constructor emits once and sub-step byte/entry updates are coalesced.
  - `reporter_reports_completion_even_inside_the_minimum_interval` — completion
    bypasses the step gate and the 100 ms interval.
  - `cancel_is_visible_to_the_reporter_and_maps_io_failures_to_cancelled` — `check`,
    `fail` and cancelled-token mapping.
- `crates/flamewm-explorer/src/thumbs/tests/mod.rs`:
  - `cache_evicts_least_recently_used_at_the_pixel_byte_budget` — four 64 KiB
    thumbnails fill `CACHE_BYTES`; the fifth evicts the oldest even though the entry
    count is far below `CACHE_CAPACITY`, and a cached failure costs no bytes.
- `crates/flamewm-task-manager/src/model.rs` (existing `mod tests`):
  - `cpu_share_is_the_busy_delta_over_the_total_delta` — 150 busy of 200 new global
    ticks reads as 75% of one core, and a regressed counter reads as 0 rather than a
    huge share.

## Verification

No `cargo` and no `git` were run (shared checkout). Compile correctness was checked by
reading each edited module: test helpers reuse the module's own imports through
`use super::*`, private fields used by the thumbnail test are reached from a descendant
module, and no production code was touched.

Status: UNVERIFIED (tests run by the orchestrator's build gate).
