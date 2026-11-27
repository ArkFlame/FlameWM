# u169 Wayland lock: dark fringe on anti-aliased edges (open)

Seen: 4x crop of the avatar ring, X11 lock vs Wayland lock (nested harness). The asset (`assets/web/lock/avatar-outline.svg`) is a pure white stroke; X11 draws a clean solid ring, Wayland draws the ring with a thin black outline that breaks into dashes along curves.

Hypothesis (unverified): the painter's RGBA is straight alpha, wl_shm `Argb8888` is premultiplied, and `rgba_to_wl_argb8888` in `crates/flamewm-lock/src/wayland.rs` only swaps R/B. Edge pixels with alpha < 255 then composite wrongly. Text edges may be affected too, which would explain the lighter text.

Acceptance: premultiply in the swap helper (add a unit test on a half-alpha pixel), rebuild with `--features wayland`, 4x crop of the ring and of the clock digits, compare against the X11 frame.

## Result of the first attempt (hypothesis rejected)
Premultiplying in `rgba_to_wl_argb8888` (colour * alpha / 255 after the R/B swap) left the black fringe unchanged in a 4x ring crop (`fix4`), so the straight-vs-premultiplied theory is wrong; reverted. `present.rs` and `template.rs` document the painter output as straight RGBA8. Next: dump the painter's RGBA for the avatar ring before the swap and look at the edge pixels' alpha; if they are opaque dark, the fringe is in the painter (or in how the Wayland path composes the avatar image over the background), not in the shm hand-off. Also confirm the X11 comparison frame came from the same settings and size.

## Findings (second pass, reading only)
- The Wayland lock does not use the X11 painter: `present_wayland` -> `present::paint_document` -> `SoftPixels` (flamewm-render-core), software `BlendImage`/`BlendMask` ops. The X11 frame is a different renderer, so the fringe is a software-renderer difference, not a shm hand-off problem.
- `PixelOp::BlendImage` documents PREMULTIPLIED input (`blend_image_span`: `out = src + dst * (1 - a)`). `prepare_sources` in `present.rs` passes `document.image_for_node` pixels through unchanged (`pixels.to_vec()`), while `RuntimeImage`/`template.rs` document them as straight. A straight white edge pixel through that formula brightens rather than darkens, so this alone does not explain the near-black edge pixels (Wayland `#010304`/`#050E12` where X11 has `#F1F3F4`/`#C5CED2` along the same row of the ring). Something paints dark pixels on the ring's edge: look for a shadow/outline mask op under the avatar image in the lowered op list for the avatar node.
Next: print the lowered `ops` for the avatar node (kind, rect, colour) in a scoped test in `crates/flamewm-lock/tests/lock_paint.rs`, then fix at the source.

## Result (fixed)
Reproduced without any compositor by painting the lock through `paint_document` directly (dark fringe on the avatar ring). Cause: `prepare_sources` handed the document's straight RGBA8 to `PixelOp::BlendImage`, which composites premultiplied pixels. Now `premultiplied()` converts first (unit-tested on a half-alpha white pixel and a transparent pixel). Direct-paint 4x crop after the fix: solid clean white ring, matching X11. 179 tests + clippy -D warnings green with --features wayland. The earlier shm-hand-off attempt was the wrong layer.
