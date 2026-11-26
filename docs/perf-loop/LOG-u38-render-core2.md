# u38-render-core2 changes

- render-core/soft `fill_rounded_rect_cpu`: a zero-alpha fill returns before touching rows, and an
  opaque fill writes the colour straight into each uncovered span (`fill_span_solid`) instead of
  running four f32 blend lookups per pixel; the blend tables are built only for translucent fills,
  which used to be built for every call including the fully opaque window/panel backgrounds.
  Composition shift: every opaque fill in the software/Wayland path drops the per-pixel table pass
  plus one 1 KiB table build per op, and invisible fills drop their whole row walk.
- render-core/pixels `composite_cpu`: the mask alpha (`round(color[3] * coverage / 255)`) is a
  function of the 8-bit coverage alone, so it is tabulated once per op (`mask_alpha_lut`) instead
  of being recomputed with a multiply and `div255` per pixel. The full-coverage write now copies a
  hoisted `[r, g, b, 255]` instead of building the array per pixel, and a zero-alpha tint skips the
  whole span. Composition shift: every text/image mask row loses one integer multiply and two
  adds per pixel; the table build is amortised over the whole op, not repeated per row.
- render-core/pixels default `paint_batch`: the shared `PixelBackend` path (the X11 CPU backend and
  any other backend that does not override it) now reuses a thread-local `BatchScratch` frame and
  region buffer instead of `pixels.to_vec()` on every batch that contains a blur, backdrop or
  clipped fill. Buffers above 16 MiB are released, so a one-off huge surface cannot pin memory.
  Composition shift: removes one full-frame heap allocation and copy per painted frame with blur or
  borders from the default CPU batch path.
- render-core/pixels `fill_clipped_cpu` and `backdrop_blur_into`: both helpers took their own
  clip-sized `Vec` per op and now take a caller-owned `region` buffer through
  `apply_ops_to_scratch`; `SoftPixels` keeps that buffer as a field and the default batch keeps it
  in the thread-local scratch. `backdrop_blur_cpu` keeps its public signature and delegates.
  Composition shift: every stroked window border and every node backdrop stops allocating a
  clip-sized buffer and copying the region twice per op.
- render-core/pixels `backdrop_blur_into`: the opaque write inside the node shape performs one
  4-byte copy and then sets alpha, instead of a 3-byte copy plus a separate alpha store, halving
  the store count of the backdrop write loop.

No visual change: the opaque fill path writes exactly what the alpha-255 lookup table produced
(every entry of that table is the source colour and 255), and the mask table is the same
`div255(color[3] * coverage)` value previously computed inline.
