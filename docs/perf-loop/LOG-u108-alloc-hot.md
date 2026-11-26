# u108-alloc-hot — per-frame allocation audit (ui-core + app draw paths)

## Audit result
Searched the ui-core sources and the four app draw/projection paths for `format!`,
`to_string()`, `.collect()` and `Vec::new()` inside code that runs per paint.

- `flamewm-ui-core/src`: no allocation in draw-adjacent code. The only `format!`s are in
  `Glass::settings_values` (settings write), `css_contract` tests and the menu builder, none of
  which run per frame.
- Explorer `app/projection.rs`: already allocation-free per row. Element ids come from the
  cached `IdTables`, per-row labels/icon/thumbnail key from `ROW_LABELS`/`LabelKey`,
  visibility from `ROW_OCCUPANCY`/`TYPE_ICON_SHOWN`, menu flags from `MENU_ENABLED`. Remaining
  `format!`s are inside open dialogs, the properties panel and the open-with chooser.
- Task manager `paint_*`: row cells use `cell_id`/`memory_text_into`/`rate_into` into reused
  buffers and `text_ref`; search uses `contains_folded`/`folded_cmp`/`pid_contains` with no
  allocation. Remaining `format!`s only run while a context/confirm menu is open.
- Terminal: `frame_rows`, `compose_row` (shared `Arc<[Run]>` handout) and the `UiCache` per-run
  text buffer are already reused. The one remaining per-redraw allocation is the multi-tab
  strip (`tab_bar` rebuilds `Run` text every frame); caching it needs a stateful title key, so
  it is left for a dedicated unit rather than risking a stale title.
- Shell taskbar panel (`projection/panel.rs`) was the one hot pass still formatting ids on
  every projection: the pager layout/cell pass and the per-slot task-progress fill.

## Changes (crates/flamewm-shell/src/projection/panel.rs, projection/async_projection/task_slots.rs)
- `project_pager_layout`: column and cell ids are written with `write!` into one reused
  `String` instead of a `format!` per node (9 columns + 18 cells + cells again in the label
  pass before this change).
- `project_pager`: one reused id buffer for the per-cell pass instead of `format!` per cell.
- `project_task_progress`: now takes the prebuilt `task-slot-<n>-progress` id; both callers
  (sync panel loop and async task-slot pass) build it in their existing reused id buffers,
  so a pass formats no per-slot fill id. Test updated to pass the literal id.
- `project_glyph_nodes`: battery-menu glyph ids come from a static `(plain, charging)` table
  (`BATTERY_MENU_GLYPH_IDS`, mirroring `BATTERY_TRAY_GLYPH_IDS`) instead of two `format!`s per
  candidate per reprojection; ids are identical to the template nodes.

## Composition shift
Idle taskbar composition was dominated by the per-second panel projection (clock tick
reprojects the panel): the id formatting contributed roughly 40 short `String` allocations
per panel pass (pager ids, 8 progress fills, battery menu glyphs) against a projection whose
remaining work is document writes. After this slice that share is gone; the same passes now
allocate only the reused scratch buffers and the already-cached `WorkspaceView` list, so the
next-ranked share in this path is document write cost, not the allocator.

## Verification
No cargo/git in the shared checkout (unit rules); correctness is by source reading. Visual
output is unchanged: every written id string is byte-identical to the previous `format!`
result, and `project_task_progress` still names `task-slot-<n>-progress`.
