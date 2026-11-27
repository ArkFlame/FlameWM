# u169 Wayland lock: dark fringe on anti-aliased edges (open)

Seen: 4x crop of the avatar ring, X11 lock vs Wayland lock (nested harness). The asset (`assets/web/lock/avatar-outline.svg`) is a pure white stroke; X11 draws a clean solid ring, Wayland draws the ring with a thin black outline that breaks into dashes along curves.

Hypothesis (unverified): the painter's RGBA is straight alpha, wl_shm `Argb8888` is premultiplied, and `rgba_to_wl_argb8888` in `crates/flamewm-lock/src/wayland.rs` only swaps R/B. Edge pixels with alpha < 255 then composite wrongly. Text edges may be affected too, which would explain the lighter text.

Acceptance: premultiply in the swap helper (add a unit test on a half-alpha pixel), rebuild with `--features wayland`, 4x crop of the ring and of the clock digits, compare against the X11 frame.

## Result of the first attempt (hypothesis rejected)
Premultiplying in `rgba_to_wl_argb8888` (colour * alpha / 255 after the R/B swap) left the black fringe unchanged in a 4x ring crop (`fix4`), so the straight-vs-premultiplied theory is wrong; reverted. `present.rs` and `template.rs` document the painter output as straight RGBA8. Next: dump the painter's RGBA for the avatar ring before the swap and look at the edge pixels' alpha; if they are opaque dark, the fringe is in the painter (or in how the Wayland path composes the avatar image over the background), not in the shm hand-off. Also confirm the X11 comparison frame came from the same settings and size.
