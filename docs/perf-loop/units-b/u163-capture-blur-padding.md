# u163 capture blur works on a padded region (open)

Area: `blur_reduced` (render/backdrop.rs) -> `frosted_backdrop_with` (flamewm-image-core/src/blur.rs).

Finding: the capture blur crops the whole reduced image and pads it by `2*radius` on every side (edge-clamped samples) so edges do not darken. At the reduced radius the padded grid is roughly 2x the area of the image, so the blur does ~2x the necessary work whenever the capture is small. Blur is ~4 ms of a capture (profile `blur_us`).

Ideas (measure the composition first, blur_us vs the others in the same run, the machine is noisy):
1. Edge-clamped blur without materialising the padding (clamp the index in the horizontal/vertical taps) - output must stay bit-identical to the oracle tests in `blur/tests`.
2. Blur only the rows/columns the later `crop_capture` reads (a window capture's crop covers the whole reduced image, so this helps only for docks/partial crops).
Acceptance: `crates/flamewm-image-core` oracle/golden tests green, frame diff 0 on the titlebar band, `blur_us` share down.

## Sizing (measured by reading, 2nd pass)
- Max reduced radius = MAX_BLUR_RADIUS_PX 24 / DESKTOP_SCALE 4 = 6.0 = HALF_DOWNSCALE_RADIUS, so the capture blur is the full-resolution three-box-pass path; pad = ceil(2*6) = 12 per side. A ~175x100 reduced window capture becomes 199x124: ~1.6x area, so dropping the padding saves ~35% of blur_us (~1.5 ms of ~35 per capture), not 2x.
- It is NOT bit-identical: three box passes reach 3*radius but the pad is 2*radius, and the padded border re-clamps; an unpadded clamp-to-edge blur differs in the outermost ~18 reduced px. Needs a visual decision (edge tint) plus a new image-core entry point; low value per risk, so deferred behind u158 and the titlebar/capture items.
