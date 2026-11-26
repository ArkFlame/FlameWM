# u64-explorer-scroll

Batch nine: 1 s of wheel scrolling a 5k-entry folder. The scout found that the glide ticker's
`WorkerResult::ScrollTick` arrives through the worker channel, so the event loop marks the wake
"changed" and runs a full window projection on every 16 ms glide frame — not the lightweight
row-move path. Rows, sidebar, toolbar, tabs, status strip and popups were re-emitted ~60 times per
second. Composition before the change, per scroll second: row text/icon/size writes ~60%, window
chrome (sidebar icons 32x11 lookups, tabs, status, menu offer) ~30%, scrollbar/rubber/other ~10%.

Changes (crates/flamewm-explorer/src/app/**):

- `projection.rs`: added a projection revision (`STATE_EPOCH`) advanced by every action, key and
  worker message except glide ticks, passive hover/motion, and wheel or scrollbar drags. A full
  chrome projection records `(revision, app, theme fills)`; while a glide is running and that key is
  unchanged, `project()` repaints only the row window and the scrollbar. Chrome (sidebar, toolbar,
  tabs, status strip, popups) is untouched because no input can have changed it.
- `projection.rs`: `RowsKey` records everything a full row pass renders (revision, origin, slot
  count, first index, day, window width, column widths, kind column, literal theme fills). A repeated
  full pass with an equal key — the common case on successive glide ticks — now runs the existing
  position-only pass that moves the rows the last pass filled.
- `projection.rs`: extracted the scrollbar into `project_scroll` so the rows-only path still updates
  the thumb, and `project_rows_positions` holds the move-only pass shared by glide frames.
- `projection.rs`: `TYPE_ICON_SHOWN` per slot. `show_type_icon` used to hide every family
  (`ROW_ICONS`, four) and size the visible one for every visible row on every full pass; it now
  returns immediately when the slot already shows that family and has no thumbnail bound. The
  thumbnail branch likewise binds, hides the type families and shows the raster only on an image-id
  change instead of on every pass.
- `projection.rs`: a row rebinds only its visible name cell; the hidden dim twin is refreshed the
  next time it shows, removing one text write per row per full pass.
- `projection.rs`: the context-menu offer pass and its 27 item visibility writes run only while a
  menu is open. A closed menu's rows live behind a hidden popup, and opening one advances the
  revision that forces this pass.
- `input.rs`: `event`/`key`/`wake` own the revision bumps with the exemptions above. Wheel and
  scrollbar drags, hover and no-drag motion leave the revision alone, so scrolling cannot be forced
  back into a full projection by pointer noise.

Expected composition shift for the same 1 s scroll: full row passes drop from ~60/s to ~one per
wheel notch, and per-tick work becomes ~40 row visibility/position writes plus the scrollbar
(~20% of the old frame). Chrome projection drops out of the scroll path entirely; icon-family
lookups drop from 4 per visible row per pass to zero while the family is stable. Visuals, labels,
selection fills, thumbnails and menu behaviour are unchanged.

Verification: static reading only (worker units run no cargo/git). `rustfmt --edition 2024 --check`
parses both edited files without errors. Build and tests are the orchestrator's gate.
