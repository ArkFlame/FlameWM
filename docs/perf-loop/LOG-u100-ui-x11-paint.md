# u100-ui-x11-paint changes

Area: `flamewm-render-x11` XRender anti-aliased paint path used by every X11 surface
(including all `flamewm-ui-x11` windows). Damage-only repaint, region paint and the
interaction-only (`hover`/`focus`) gate were already in place from earlier batches, so this
pass targets the remaining per-pixel request amplification inside a single painted primitive.

- `render-x11/xrender_aa` stroke batching: `stroke_rounded_rect_clipped` issued one XRender
  Composite per run per row for the whole rect. Straight rows repeat one identical run list, so a
  400 px tall window border cost ~800 requests and ~400 `Vec` allocations per paint. Consecutive
  rows whose run list is equal are now merged into one rect of the span (`paint_span`), and the
  row list is rebuilt into two reusable scratch vectors. Pixel equality holds because merged rows
  share x, width, coverage colour and are vertically adjacent; the clipped path intersects the
  span with the clip vertically and each run horizontally, exactly as the old row loop did.
  Composition shift: bordered panels/windows drop from O(height) requests to O(radius) per
  stroke; a radius-0 square border drops from `height` requests to one. Largest share removed:
  client request building plus server-side composite dispatch inside the paint span.
- `render-x11/xrender_aa` corner fills: `fill_rounded_rect` allocated one `Vec` per corner row
  (`fill_row_runs`). It now refills one scratch vector through `fill_row_runs_into` /
  `stroke_row_runs_into`, so the existing per-row fill keeps its request count but loses 2 *
  radius allocations per translucent rounded fill (panels, cards, buttons, dock, popups).
  Composition shift: removes allocator traffic from every region repaint that touches a rounded
  fill; the draw ops are unchanged.

## Review notes

- The public row-run helpers (`fill_row_runs`, `stroke_row_runs`) keep their signatures and are
  still used by `accel` (GPU mask build) and the existing AA tests; only the allocation and the
  row loop changed, so retained visuals are identical by construction.
- No cargo/git in the shared checkout (per unit rules); correctness is by source reading, the
  orchestrator builds and runs the AA tests.

## Skipped

- A8 mask-picture caching per `(width, height, radius)` would take a rounded fill from
  O(radius) requests to one, but it adds new X pixmaps/pictures and changes corner rounding by
  one alpha step; rejected for this pass to keep pixel-identical output without a build.
