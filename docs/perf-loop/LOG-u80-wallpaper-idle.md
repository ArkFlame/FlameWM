# u80 wallpaper idle log

Fifth-pass scout of the desktop wallpaper/background path (`crates/flamewm-desktop/src/wallpaper_loader.rs`,
`projection.rs`, `wallpaper_stamp.rs`, `main.rs`). Earlier passes already put decode on one bounded
worker, shared the pixel buffer through `Arc`, fenced completions by generation, and skipped the
image upload when the stamp is unchanged. The composition of a static desktop second in this area is
dominated by two remaining mechanisms: a full settings-projection pass per wallpaper wake, and a
full decode + compose + root-sized upload per repeated request.

- main/flush (wake handler): the wallpaper wake now drains and `poll`s the completion inside the
  handler, and arms `settings_visual_dirty` only for `Applied`/`Failed`. A `Stale` completion (a
  generation superseded by a newer request, common while dragging fit/mode sliders) or an
  already-drained wake no longer runs the whole projection pass (menus, widgets, accent tokens,
  sticky, cursor theme, render-acceleration gate) that a plain `true` used to force.
  Composition shift: per-wake projection work drops to zero for wakes that carry no new image;
  only an applied raster or a new diagnostic repaints.
- wallpaper_loader: `request_layout` is idempotent for identical inputs. A request whose
  path/fit/`share`/monitor rectangles equal the pending request returns that generation, and a
  request equal to the one that produced the presented image returns the applied generation. In
  both cases no worker turn is staged and no new generation is allocated, so the already-decoded
  `Arc<Vec<u8>>` is reused. Failure and solid paths do not populate/retain that identity, so a
  retry after a failed load, or a return from solid to the same image, still decodes once.
  Composition shift: startup, topology-refresh and duplicate settings signals decode and upload one
  raster once instead of once per request; a redundant request's share (decode CPU on the worker
  plus a full-frame RGBA upload on the UI turn) goes to zero.
- Verified unchanged: the decoded pixels have exactly one owner. The worker moves its `Vec` into
  `Arc`, `poll` moves that `Arc` into the presentation, and the document keeps the same allocation
  via `image_rgba8_shared`, so dropping the loader's handle after upload would free nothing; no
  second pixel buffer exists to release.
- Verified unchanged (feature, not regression): no slideshow/rotation feature or timer exists in
  the tree (no `slideshow`/`rotation` reads in `flamewm-desktop`, settings, or image core), so
  there is no disabled timer to park. The one wallpaper thread blocks on its condvar with no
  deadline while the request slot is empty, and is woken only by an actual request.

Tests: `identical_layout_request_reuses_the_decoded_raster` adds the idempotent-request contract
(returned generation, no staged completion, same pixel allocation; a changed fit decodes again).
