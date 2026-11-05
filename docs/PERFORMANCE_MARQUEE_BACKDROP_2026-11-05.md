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

## Phase breakdown of one desktop recapture (instrumented, 2026-11-05)

Source: `render/backdrop.rs` `PhaseClock`, enabled with `FLAMEWM_BACKDROP_PROFILE=1` (read once; off = one
cached bool test per capture, no clock, no allocation). One `BACKDROP_PHASE` line per capture on the
compositor stderr. Run: `FLAMEWM_BACKDROP_PROFILE=1 scripts/perf/fm.sh <bin> marquee`, 49 captures, nested
session output 1280x641 (`rows=641`, 21 readback strips of 32 rows).

| Phase | What it covers | Median us | Mean us | Share of median total |
| --- | --- | ---: | ---: | ---: |
| `copy` | per strip: `GenBuffers`+`BufferData`+`ReadPixels` into PBO; the first readback likely also waits for the GPU render (inferred, not isolated) | 3613 | 3687 | 39% |
| `blur` | `frosted_backdrop` on the quarter-size image (flamewm-image-core) | 1888 | 1934 | 20% |
| `downscale` | per strip: CPU box average into the quarter-size image | 1851 | 1869 | 20% |
| `texture` | `renderer.create_buffer` (full-output GL texture allocated per capture) | 1576 | 1607 | 17% |
| `render` | `OutputDamageTracker::render_output` into that texture (CPU command recording) | 206 | 1798 | 2% |
| `map` | `map_texture` (PBO map) | 25 | 36 | <1% |
| `elements`, `store` | element commit collection, `set_desktop` | 138, 19 | 225, 20 | <2% |
| `total` | whole capture | 9358 | 11180 | 100% |

Notes:
- Median total is ~9.4 ms, not the ~27 ms in the 15d8ff93 note. That note was taken on a loaded machine
  (load ~25). This run's load was ~5 to 7. Read the shares, not the absolute numbers.
- `render` has a long tail (max 54.9 ms on the first captures after startup). The steady-state median is 0.2 ms.
  Render cost likely lands in `copy`, where the readback forces the GPU to finish (inferred from the split, not isolated).
- Compositor CPU in the marquee run was 88 to 122% total. Capture accounts for roughly 10% of that.

## Candidate fixes and A/B result

| Candidate | Outcome |
| --- | --- |
| Box-average downscale as contiguous per-row column sums, bit-identical | Kept out. Unit test vs the old per-pixel reference passed (ragged sizes, mid-block strips). fm.sh marquee A/B, 3 alternating pairs, compositor CPU%: base 121.96, 90.92, 91.81 (median 91.81); new 88.01, 94.10, 95.10 (median 94.10). No repeatable win, below the 3% bar, reverted with its test. |
| Reuse the full-output GL texture across captures | Not implemented. Saves ~1.6 ms per capture, but keeps a full-output RGBA texture resident between captures (about 3.3 MB at 1280x641, 8 MB at 1080p). That is a RAM-for-CPU trade. Ceiling is ~1.6 of ~9.4 ms, about 1.5 points of compositor CPU. Not measured. |
| One full-output readback instead of 32-row strips | Not implemented. It would remove per-strip PBO setup and stalls, but needs a transient full-size readback buffer, which reverses the deliberate strip design. Not measured. |
| Skip recapture while only the rubber band changes | Not implemented. The band is a desktop-layer commit, so every motion changes the commit set. Skipping it changes pixels under chrome while dragging. That is a quality change, so it needs a design decision. Not measured. |

The blur (20%) lives in `flamewm-image-core`, which this change does not own. The remaining candidates
above each need a decision or a RAM trade before they can be measured. This change keeps only the
env-gated profiler.

## Verification

- `cargo test -p flamewm-wm-wayland`: exit 0, 203 passed, 0 failed.
- `cargo check --workspace --all-targets`: see the final report.
- Not run: `fm.sh idle`/`hover`, DRM backend, any runtime behavior beyond the marquee profile run.

## Follow-up: texture reuse kept, single readback reverted (coordinator policy, 2026-11-05)

Policy from the coordinator: do not trade CPU for RAM. A ~3 MB resident capture texture is accepted.
Judged by the `BACKDROP_PHASE` median per-recapture total, alternating runs, `fm.sh marquee` with
`FLAMEWM_BACKDROP_PROFILE=1`. Machine load moved between batches (`prof` ~12.2 ms, `tex` ~10.3 ms in batch 1; `tex` ~11.6 ms, `texr` ~11.9 ms in batch 2), so compare within a batch only.

| Variant | Batch | Samples | Median total us | Median copy us | Median texture us | Verdict |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| `prof` = 5f53c437 (per-capture texture, 32-row strips) | 1 | 141 | 12187 | 4764 | 1868 | base |
| `tex` = scratch texture reused across captures | 1 | 143 | 10286 | 4671 | 0 | **kept, -15.6%** |
| `tex` | 2 | 138 | 11578 | 5082 | 0 | base |
| `texr` = `tex` + one full-output readback | 2 | 139 | 11905 | 5813 | 0 | **reverted, +2.8%** (copy +14%) |

Implementation of the kept change:
- `Decorations::scratch` (`render/titlebar.rs`) holds `((w, h), GlesTexture)`, output-sized. It is reused while the size matches. It is replaced on resize, and it is kept across `drop_desktop`, so it stays resident while the decorations exist (~4 bytes per output pixel, about 3.3 MB at 1280x641).
- `take_scratch` (`render/backdrop.rs`) takes the matching texture or drops a mismatched one. Unit test `scratch_tests::scratch_is_reused_only_for_the_same_output_size` covers the three cases.
- The full output is still rendered with `CLEAR` and full damage, so a reused texture is fully overwritten. Output bits are unchanged.

Sanity pair (`fm.sh marquee`, no profiler env, one pair): base 86.83% vs tex 87.92% compositor CPU, Anon 44.2 vs 47.7 MB. This is inside noise. The phase total implies a ~1.5-point CPU effect, which this pair cannot resolve.

Single readback: reverted. Copy time rose because one 3.3 MB PBO readback was slower than 21 strips of 160 KB here. The strip loop stays.

Verification: `cargo test -p flamewm-wm-wayland` exit 0 (204 passed, 0 failed). `cargo check --workspace --all-targets` exit 0, 0 warnings.
Not run: `fm.sh idle`/`hover`, DRM path, repeated fm.sh pairs for CPU% (CPU% is too noisy at this effect size).

## Negative results index (2026-11-05)

- Single full-output readback: reverted, +2.8% total, copy +14% (table above). Strip loop kept.
- Paint coalescing, retained backbuffer, incremental blur: listed as negative by the coordinator brief. No measurement, commit, or verdict for these was found in this repo (`docs/`, `.agents/`) on 2026-11-05. Record the measured numbers here before citing them.
