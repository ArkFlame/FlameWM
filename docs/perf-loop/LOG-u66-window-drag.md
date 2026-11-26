# u66 window drag/resize changes

Area: 1 s of interactive window move/resize on the compositor path that the WM and the app windows
share. `git log` is unavailable in the shared checkout, so the review read the current sources and
the earlier render/backdrop logs (u22, u30, u37, u48) rather than the diffs. The three paths that
still pay per drag sample were the WM ring upload, the full backdrop snapshot re-upload on every
move, and the full mirrored-fill re-upload on every resize frame.

- render-x11 `outline_overlay`: `present` premultiplied every strip into a fresh `Vec<u8>` and issued
  one `XCreateImage` + `XPutImage` per strip on every frame paint. The last uploaded strips are now
  retained (rect, size, premultiplied bytes) and a strip whose rect and pixels match is skipped
  entirely; `hide` forgets them because an unmapped window's contents are not guaranteed. The WM
  re-presents the ring from `paint_outline_overlay`, which runs on every frame repaint, so a move
  that only reconfigures the ring window now uploads nothing, and a resize uploads just the two
  bands that changed. Composition shift: the ring's per-paint cost during a drag drops from four
  premultiply passes plus four image uploads to a byte compare.
- render-x11 `app/window_backdrop` (`capture_window_backdrop` + `patch_moved_window_backdrop`,
  `shift_patches`, `crop_words`): a moved window re-cropped the same canvas and re-uploaded the full
  window-sized snapshot pixmap, although only the strips the shift exposed differ. When the size,
  canvas epoch and retained pixmap all match, the capture now uploads just those strips (left/right
  plus top/bottom, at most two rects) and keeps the pixmap for the overlap; a whole-window jump or a
  resize still takes the full path. Composition shift: dragging a translucent window drops the
  per-sample snapshot upload from `w*h*4` bytes to `(|dx|*h + dy*w)*4`.
- render-x11 `app/surface_backdrop` (`paint_mirrored_window_backdrop`, `MirrorFill.capacity`/
  `filled`, `mirror_capacity`, `upload_argb_pixmap_sized`, `mirror_extend_argb_rect`): an
  interactive resize keeps the snapshot stale until it settles, so the mirrored fill ran every
  frame. It recomputed the whole window and allocated and uploaded a new full-size pixmap for every
  new `(size, origin)` key. The fill now keeps one pixmap with a quarter slack in each axis and
  records how much of it is written for the current origin; growing uploads only the appended
  strips, an unchanged key blits with no upload, and a changed origin or a shrink refills the key
  area in place. Because the mirror of a pixel depends on its coordinates and the origin only,
  retained pixels stay valid across a regrow, so `filled` is always one rectangle. Composition shift: a per-frame resize sample drops from a fresh
  2.5 MB-class pixmap allocation and upload to two thin strips inside one retained allocation.
  `store_mirror_fill` now only frees a pixmap it actually replaced, since a grown fill keeps the
  same handle.
- render-x11 `backdrop/extend`: `mirror_extend_argb` is now the full-canvas case of
  `mirror_extend_argb_rect`, with a test that every sub-rect equals the full canvas at the same
  coordinates. `app/images`: `upload_words_image_at` (and the app-level `upload_argb32_words_at`)
  place an image at a pixmap offset; the old `upload_words_image` is the `(0, 0)` case.
- render-x11 `app/window_backdrop` (`mark_window_backdrop_stale`): it bumped the backdrop
  generation on every ConfigureNotify, which invalidated backbuffer reuse even when the recapture
  turned out to be the same pixels. The capture path already bumps the generation exactly when the
  cropped words change, so the event-time bump is gone. Visual result is unchanged: a deferred
  recapture still shows the same stale snapshot a forced repaint would have drawn.
- wm-x11 `wm/paint.rs` (compile fix only, not an optimization): `OutlineOverlay::present` needs
  `&mut self`, so `configure(...).and_then(|()| present(...))` became an explicit match. No
  behavioural change.

Not touched: `crates/flamewm-desktop/src/**`. The desktop is a full-root window with no configure
stream during another window's drag, so it contributes no work to this sample; its drag and sticky
paths were already flattened in u30. Dead code: no speculative deletion, because the shared
checkout cannot run the compiler and an unused-warning sweep here would be guesswork.

Checks: no cargo/git per the unit brief. Every edited file passes `rustfmt --check` (parse + format)
on edition 2024; the new pure helpers carry unit tests (`shift_patches`, `crop_words`,
`mirror_capacity`, `mirror_extend_argb_rect`).
