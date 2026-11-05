# Marquee desktop backdrop: incremental recapture, measured and reverted (2026-11-05)

Status: no code change kept. The incremental-blur path is not worth implementing.

## Evidence
- Desktop capture (`refresh_desktop_backdrop`, `render/backdrop.rs`) costs ~27 ms per run and runs every
  100 ms while the desktop rubber band commits (`DESKTOP_RECAPTURE_INTERVAL`).
- Blur share, measured on the loaded machine (release, `frosted_backdrop` on a 480x270 quarter-size image,
  desktop radius = 24 px * 80% / 4 = 4.8 px, 20 calls averaged): radius 2 = 1.72 ms, radius 5 = 2.73 ms,
  radius 10 = 2.92 ms. Blur is ~10% of one capture.
- Upper bound: capture is at most 27% of one core during a marquee, so removing the blur entirely saves
  at most ~2.7 points of a core. Marquee CPU in prior runs was 89% to 142%, so the ceiling is ~2% of total.
- A partial blur is bounded by the same ceiling and falls back to full above 50% damage. The ellipse marquee
  in `scripts/perf/fm.sh` covers much of the output, so that fallback would often run.
- The 27 ms is mostly GPU render, full-output readback and downscale. Cutting it needs persistent full-output
  GPU state or a partial readback. That is a larger change and is out of scope here.

## Bit-identity constraint
`blur_stage` box-downsamples by 4 and runs three horizontal and vertical running-sum passes with f32 state.
A partial recompute must replay each affected column from its top row and each affected row from its left
edge to stay bit-identical to a full capture. That makes the win smaller still.

## Decision
Below the >=3% bar, so reverted. No fm.sh A/B was run because the analytic ceiling is already under the bar.
The 2.7 ms blur measurement is blur-only, not end-to-end.
