# z04: glass and blur cost — remaster design

## Scope and evidence

Unit: `docs/swarm/wave5/z04-blur-glass-cost.md`. Canonical blur owner is the
`flamewm-image-core` blur module; `flamewm-render-core` consumes it only through
the typed `PixelBackend` facade. No native or GPU access is added, no gate is
loosened, no dependency is added, and emitted pixels are unchanged.

Reference-first evidence (read-only, never a dependency or runtime input):

- `.vendor/kwin-master/src/plugins/blur/blur.cpp:211-267` builds a static blur
  strength table. `blurOffsets` at lines 238-244 enumerates downsample levels
  `/2`, `/4`, `/8`, `/16` with `{minOffset, maxOffset, expandSize}` and stops
  before `/32` and `/64`. `m_iterationCount` (line 274) is the number of
  downsample iterations for a strength; `m_expandSize` (line 276) bounds the
  copy region. The dual Kawase passes are described at lines 772 and 799; the
  shaders are `shaders/downsample.frag` and `shaders/upsample.frag`.
- `.vendor/cosmic-epoch/cosmic-comp/src/backend/render/wayland/blur_effect.rs:36-80`
  computes the same table in Rust: `MAX_STEPS = 15`, `BlurParameters { passes,
  offset, extended_radius }` and `offsets = [(1,2,10), (2,3,20), (2,5,50),
  (3,8,150)]`. `BLUR_PARAMS[strength]` is consumed at line 227 for the capture
  radius and at lines 278-284 for pass state.
- Choice: port the *table enumeration* idea, not the Kawase shaders or their
  offsets. Our CPU blur is a three-box Gaussian approximation on grid factors
  `1/2/4`, so the typed table must describe the grid factor and box radii we
  actually run, not the vendor's offset range. This keeps pixels identical.

## Current structure (file:line)

| Path | Line | Role |
| --- | --- | --- |
| `crates/flamewm-image-core/src/blur.rs` | 14-24 | `MAX_BLUR_RADIUS`, `MIN_BLUR_RADIUS`, `DOWNSCALE_RADIUS` (12), `HALF_DOWNSCALE_RADIUS` (6), `DOWNSCALE_FACTOR` (4) |
| `crates/flamewm-image-core/src/blur.rs` | 51-61 | `BlurScratch` work buffers (`work`, `down`, `line`, `band`, `taps`) |
| `crates/flamewm-image-core/src/blur.rs` | 117-148 | `gaussian_blur_rgba` / `_with`: validate, `blur_stage`, `blur_emit` |
| `crates/flamewm-image-core/src/blur.rs` | 155-250 | `frosted_backdrop` / `_with`: padded region, same `blur_stage` |
| `crates/flamewm-image-core/src/blur/kernel.rs` | 46-76 | `blur_stage`: chooses grid, sigma shrinks by factor |
| `crates/flamewm-image-core/src/blur/kernel.rs` | 78-89 | `blur_factor`: 1 / 2 / 4 threshold table |
| `crates/flamewm-image-core/src/blur/kernel.rs` | 136-173 | `blur_boxes` and `box_radii`: three box passes, Kutskir variance matching |
| `crates/flamewm-image-core/src/blur/kernel.rs` | 194-290 | `box_horizontal`, `box_vertical`: two sliding-window passes per radius |
| `crates/flamewm-image-core/src/blur/kernel.rs` | 292-... | `box_downsample_rows`: grid reduction before the box passes |
| `crates/flamewm-render-core/src/pixels.rs` | 115-143 | `PixelOp::Blur` and `PixelOp::BackdropBlur` typed facade |
| `crates/flamewm-render-core/src/pixels.rs` | 226-300 | `backdrop_blur_cpu` / `backdrop_blur_into` call `PixelBackend::blur_rgba` |
| `crates/flamewm-render-core/src/pixels.rs` | 579, 706, 715-720, 783 | backend implementations dispatch blur ops |
| `crates/flamewm-render-core/src/soft.rs` | 18, 51-58 | software backend owns a `BlurScratch` |
| `crates/flamewm-render-core/src/pixels.rs` | 1040-1063 | existing scoped blur benches (r8, r24 downscaled, backdrop) |

## Counted problems

1. The pass and downsample decision lives only inside `blur_stage`
   (`kernel.rs:46-76`) and `blur_factor` (`kernel.rs:78-89`). There is no typed,
   callable description of "which passes run at this radius", so cost cannot be
   inspected, budgeted, or tested without reading private kernel internals.
2. Vendor references expose a strength-to-pass table (`kwin:238-276`,
   `cosmic:47-80`); our counterpart has no table and no test asserting that the
   accounting matches the kernel actually used.
3. Radius work per frame is unbounded at the facade: `PixelOp::Blur { radius }`
   (`pixels.rs:116`) forwards any accepted radius up to `MAX_BLUR_RADIUS`. No
   per-frame budget exists yet.
4. Number of problems: 3. Problem 3 is deliberately not fixed in this slice.

## Target design

Introduce a typed pass table owned by the blur module, then let the facade use
it for accounting and eventually for a per-frame cap. The table is derived from
the same `blur_factor` and `box_radii` that drive `blur_stage`, so it cannot
drift from emitted pixels. No pixel path is modified in any step.

`BlurPassTable { downsample: usize, radii: [usize; 3] }` with
`separable_passes()` and `resample_passes()`; `blur_pass_table(radius)` is a pure
constructor in the domain accepted by `gaussian_blur_rgba`.

## Steps (each compiles alone)

### Step 1 — typed pass table and tests (implemented here, UNBUILT)

Add `BlurPassTable` and `blur_pass_table` to
`crates/flamewm-image-core/src/blur.rs`, plus three tests in
`crates/flamewm-image-core/src/blur/tests.rs` asserting: downsample thresholds,
radii equal to `box_radii` on the shrunken grid, and bounded pass counts
(at most six separable and two resampling passes). Additive and pure: no pixel
changes, no gate changes, no new dependency. This step compiles alone.

### Step 2 — facade accounting (not implemented)

Re-export the table through the `flamewm-render-core` typed facade and have
`backdrop_blur_cpu` count per-frame work in a frame-scoped accumulator. No
pixel change. Compiles alone on top of step 1.

### Step 3 — per-frame radius cap (not implemented)

Use the accumulated pass work to reduce the effective radius of later blur ops
in the same frame, behind the facade and with an explicit configuration knob.
This changes which radius is requested, not the blur kernel. Requires visual
review because a capped radius changes appearance; it is not part of this unit.

## Rollback

Revert step 1 by deleting the `BlurPassTable` block and `blur_pass_table` from
`blur.rs` and the three tests from `tests.rs`. Nothing else references them, so
the rollback is a clean delete with no pixel or behavior impact. Steps 2 and 3
(if applied later) must be reverted in reverse order before step 1.

## Verification limits

Builds, tests, and commits are owned by the orchestrator and were not run here.
All edits in this slice are UNBUILT. No runtime or performance claim is made.
