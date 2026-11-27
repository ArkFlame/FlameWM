# u163 capture blur works on a padded region (open)

Area: `blur_reduced` (render/backdrop.rs) -> `frosted_backdrop_with` (flamewm-image-core/src/blur.rs).

Finding: the capture blur crops the whole reduced image and pads it by `2*radius` on every side (edge-clamped samples) so edges do not darken. At the reduced radius the padded grid is roughly 2x the area of the image, so the blur does ~2x the necessary work whenever the capture is small. Blur is ~4 ms of a capture (profile `blur_us`).

Ideas (measure the composition first, blur_us vs the others in the same run, the machine is noisy):
1. Edge-clamped blur without materialising the padding (clamp the index in the horizontal/vertical taps) - output must stay bit-identical to the oracle tests in `blur/tests`.
2. Blur only the rows/columns the later `crop_capture` reads (a window capture's crop covers the whole reduced image, so this helps only for docks/partial crops).
Acceptance: `crates/flamewm-image-core` oracle/golden tests green, frame diff 0 on the titlebar band, `blur_us` share down.
