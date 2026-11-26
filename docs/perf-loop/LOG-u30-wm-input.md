# u30-wm-input changes

- desktop/drag (`events.rs`, `drag.rs`, `state.rs`, `main.rs`): item-drag motion used to run the
  whole ghost projection per input event — label wrap, count `format!`, a 44px icon raster clone
  plus `fade_ghost_pixels`, an `image_rgba8` upload, a dozen geometry setters and a 128-slot
  source-tile scan. Motion now only stages `drag_pending`; the flush turn applies the latest
  pointer once. Constant chrome (labels, sizes, font sizes, source hiding, icon) is projected on
  the first flush only, later flushes write one position. Composition shift: a fast drag costs one
  ghost position write per frame instead of one full ghost rebuild per pointer sample.
- desktop/drag (`drag.rs`, `projection.rs`): the ghost icon is resolved from the warm cache once
  and marked `icon_resolved`; a cache miss stays on the Flame fallback and `sync_drag_ghost`
  swaps in the drained raster. Previously every motion cloned and faded the raster again.
- desktop/sticky (`sticky.rs`, `main.rs`): note move/resize motion called `sync_sticky` per event
  (every visible note: position, size, background, caret text, font, weight, foreground, five
  visibility writes). Motion/resize now stage `pending_pointer`; `apply_pending_gesture` applies
  the latest pointer once per flush and only syncs when the preview rect actually changed.
- desktop/sticky (`sticky.rs`): `sync_sticky` keeps a per-slot shown flag, so an empty slot is
  cleared once instead of writing 8 nodes every sync, and a non-editing note's text is written
  with `text_ref` (no `String` clone per note per sync).
- desktop/slots (`slots.rs`): while a ghost exists, unchanged tiles no longer re-assert visibility
  for all 128 slots with a per-slot `format!`. The tile projection that recorded the inputs already
  left the slot visible, and `sync_drag_ghost` keeps the ghost's hidden source tiles hidden, so the
  assertion was pure per-sync allocation and document writes.
- desktop/input (`input.rs`, `events.rs`): convert-wheel hover hit-tests built the full label list
  (eight `String`s, `offered_formats` + uppercase per event) just to read its length. A new
  `wheel_slot_count` returns the count directly. `update_wheel_hover` caches the uploaded
  328x328 sector raster by `(sector, accent)`, so re-entering the same sector or refreshing the
  same accent no longer regenerates and uploads a full RGBA layer; the cache is dropped when the
  highlight hides.
- desktop/output_layout (`output_layout.rs`): replaced the `expect("every model item is
  projected")` panic path with a total fallback cell, keeping the projection aligned without a
  crash route.
